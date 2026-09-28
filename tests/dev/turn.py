"""One CLI turn against a fake model, timed and inspected.

    python3 tests/dev/turn.py            # all checks
    python3 tests/dev/turn.py --hold 30  # a host that holds the stream longer

What it answers, each as PASS or FAIL, exit 1 on any failure:

  - a turn ends when the model's answer does, not when the connection
    closes. A host that keeps the connection open after `[DONE]` once held
    every step of every run for as long as it pleased (a 2.3 s reply took
    22 s), which is what "slow with this provider" looked like from outside;
  - the API key reaches the provider, is never on a command line while the
    request runs (other processes can read those), and leaves no header
    file behind;
  - a refused key is reported with where to fix it.

Needs `cli` built (cd core && cargo build --bins) and curl on PATH.
"""

import argparse
import os
import subprocess
import sys
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from devkit import Report, binary, matching, scratch  # noqa: E402
from fakemodel import FakeModel  # noqa: E402

KEY = "sk-devcheck-%d" % os.getpid()


def turn(cli, env, text="hello", timeout=120):
    """Send one message; return (seconds to the answer, every output line)."""
    p = subprocess.Popen([cli], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                         text=True, env=dict(os.environ, **env), cwd=scratch("cwd"))
    t0 = time.time()
    p.stdin.write("say %s\nmark done\n" % text)
    p.stdin.flush()
    lines, answered = [], None
    deadline = t0 + timeout
    for line in p.stdout:
        lines.append(line.rstrip())
        if answered is None and (line.startswith("ANSWER") or line.startswith("STOPPED") or line.startswith("ERROR")):
            answered = time.time()
        if "RECEIPT mark=done" in line or time.time() > deadline:
            break
    p.kill()
    p.wait()
    return answered, lines


def main():
    ap = argparse.ArgumentParser(description="Time and inspect one CLI turn against a fake model.")
    ap.add_argument("--hold", type=float, default=15.0, help="seconds the fake host holds the stream after [DONE]")
    a = ap.parse_args()
    cli = binary("cli")
    r = Report("one turn against a fake model")

    # 1. An ordinary host, which closes when it is done.
    with FakeModel(delay=0.3) as m:
        home = scratch("turn")
        answered, lines = turn(cli, m.env(home, KEY))
        ok = answered is not None and m.finished
        lag = (answered - m.finished[0]) if ok else None
        r.check("the answer arrives", ok, "no ANSWER line" if not ok else "")
        if ok:
            r.check("the turn ends with the stream", lag < 1.5, "%.2fs after the model's last token" % lag)

    # 2. A host that keeps the connection open after [DONE].
    with FakeModel(delay=0.3, hold=a.hold) as m:
        home = scratch("hold")
        answered, lines = turn(cli, m.env(home, KEY))
        if answered is not None and m.finished:
            lag = answered - m.finished[0]
            r.check("a held connection does not hold the turn", lag < 3,
                    "%.2fs after the last token, with the host holding %.0fs" % (lag, a.hold))
        else:
            r.check("a held connection does not hold the turn", False, "no answer at all")

    # 3. The key: sent, never on a command line, no file left behind.
    with FakeModel(words=["slow "] * 6, delay=0.5) as m:
        home = scratch("key")
        seen_on_argv = []
        stop = threading.Event()

        def watch():
            while not stop.is_set():
                seen_on_argv.extend(c for _, c in matching(KEY))
                time.sleep(0.15)

        w = threading.Thread(target=watch, daemon=True)
        w.start()
        turn(cli, m.env(home, KEY))
        stop.set()
        w.join()
        auths = [q["auth"] for q in m.requests if q["path"].endswith("/chat/completions")]
        r.check("the key reaches the provider", bool(auths) and all(x == "Bearer " + KEY for x in auths), repr(auths))
        r.check("the key is never on a command line", not seen_on_argv,
                "seen in: " + seen_on_argv[0][:120] if seen_on_argv else "")
        left = [f for f in os.listdir(home) if f.startswith(".bearer")] if os.path.isdir(home) else []
        r.check("no key file is left behind", not left, ", ".join(left))

    # 4. A refused key says where to fix it.
    with FakeModel(status=401) as m:
        home = scratch("401")
        _, lines = turn(cli, m.env(home, KEY))
        said = next((l for l in lines if "401" in l), "")
        r.check("a refused key says where to fix it", "Settings" in said, said[:160] or "no line mentions the 401")

    return r.done()


if __name__ == "__main__":
    sys.exit(main())
