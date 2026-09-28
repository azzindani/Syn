"""A fake OpenAI-compatible model, for measuring Syn rather than a provider.

Streams a fixed answer word by word, and can misbehave on purpose in the
ways real hosts do: answer slowly, hold the connection open after `[DONE]`
(keep-alive, a proxy, a gateway flushing late), or refuse the key. It
records what each request carried -- the Authorization header, the model --
so a check can ask what Syn actually sent.

Standard library only. Use it from another tool:

    with FakeModel(words=["Opened ", "it."], hold=20) as m:
        ... point AGENT_BASE_URL at m.url ...
        m.requests  # [{"auth": "Bearer ...", "model": "...", "path": "..."}]

or on its own, to point a console at while you watch:

    python3 tests/dev/fakemodel.py --port 8099 --delay 0.5 --hold 20
"""

import argparse
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class FakeModel:
    def __init__(self, words=("Opened ", "the ", "workbook."), delay=0.3, hold=0.0, status=200, port=0, think=0.0):
        self.words = list(words)
        self.delay = delay
        # Seconds of silence before the first word: a model reasoning without
        # streaming its thoughts. A client killed now has nothing arriving to
        # fail on, which is when an orphaned curl waits out its timeout.
        self.think = think
        # Seconds to keep the connection open after `[DONE]`: how a host that
        # never closes looks from the client. Syn must not wait for it.
        self.hold = hold
        self.status = status
        self.requests = []
        # When the last token went out, per request, so a caller can tell
        # the model's time from Syn's.
        self.finished = []
        self._server = ThreadingHTTPServer(("127.0.0.1", port), self._handler())
        self._server.daemon_threads = True
        self._thread = None

    @property
    def url(self):
        return "http://127.0.0.1:%d" % self._server.server_address[1]

    def env(self, home, key="sk-fake-model-key"):
        """The environment a CLI or console needs to talk only to this."""
        return {
            "AGENT_ENV_FILE": home + "/no.env",
            "AGENT_HOME": home,
            "AGENT_API_KEY": key,
            "AGENT_BASE_URL": self.url,
            "AGENT_BASE_URL_STANDARD": self.url,
            "AGENT_API_KEY_STANDARD": key,
            "AGENT_MODEL_STANDARD": "fake/model",
        }

    def __enter__(self):
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return self

    def __exit__(self, *exc):
        self._server.shutdown()
        self._server.server_close()

    def _handler(self):
        model = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args):
                pass

            def do_GET(self):
                # The console asks every provider for its model list.
                body = json.dumps({"data": [{"id": "fake/model", "context_length": 32000}]}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_POST(self):
                raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
                try:
                    asked = json.loads(raw or b"{}").get("model", "")
                except ValueError:
                    asked = ""
                model.requests.append({"auth": self.headers.get("Authorization", ""), "model": asked, "path": self.path})
                if model.status != 200:
                    body = json.dumps({"error": {"message": "Invalid API key", "code": model.status}}).encode()
                    self.send_response(model.status)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.send_header("Connection", "close")
                    self.end_headers()
                    self.wfile.write(body)
                    return
                try:
                    self._stream()
                except (BrokenPipeError, ConnectionResetError):
                    # The client went away mid-answer: killed, stopped, or
                    # done reading. For these tools that is often the point.
                    self.close_connection = True

            def _stream(self):
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.flush()
                time.sleep(model.think)
                for w in model.words:
                    self._event({"choices": [{"delta": {"content": w}}]})
                    time.sleep(model.delay)
                self._event({"choices": [{"delta": {}, "finish_reason": "stop"}]})
                self.wfile.write(b"data: [DONE]\n\n")
                self.wfile.flush()
                model.finished.append(time.time())
                if model.hold:
                    time.sleep(model.hold)
                self.close_connection = True

            def _event(self, obj):
                self.wfile.write(("data: %s\n\n" % json.dumps(obj)).encode())
                self.wfile.flush()

        return Handler


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--port", type=int, default=8099)
    ap.add_argument("--delay", type=float, default=0.3, help="seconds between words")
    ap.add_argument("--hold", type=float, default=0.0, help="seconds to keep the connection after [DONE]")
    ap.add_argument("--think", type=float, default=0.0, help="seconds of silence before the first word")
    ap.add_argument("--status", type=int, default=200, help="answer every request with this status (401 to refuse the key)")
    a = ap.parse_args()
    with FakeModel(delay=a.delay, hold=a.hold, status=a.status, port=a.port, think=a.think) as m:
        print("fake model at %s -- set AGENT_BASE_URL=%s and any AGENT_API_KEY; Ctrl+C to stop" % (m.url, m.url))
        try:
            while True:
                time.sleep(3600)
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
