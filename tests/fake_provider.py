"""Fake OpenRouter endpoint: captures POST body, replies canned JSON. Stdlib only."""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

CAPTURE = sys.argv[2] if len(sys.argv) > 2 else "/tmp/fake-body.json"


class H(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n)
        with open(CAPTURE, "wb") as f:
            f.write(body)
        try:
            data = json.loads(body)
            assert "model" in data and "messages" in data, "bad body shape"
        except Exception as e:  # noqa - report failure via status
            self.send_response(400)
            self.end_headers()
            self.wfile.write(("shape: %s" % e).encode())
            return
        resp = b'{"choices":[{"message":{"content":"ok"}}]}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(resp)))
        self.end_headers()
        self.wfile.write(resp)

    def log_message(self, *a):
        pass


HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
