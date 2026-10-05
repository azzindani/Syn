"""Kill Syn outright in the middle of its work, and see what is left running.

    python3 tests/dev/leftovers.py

A clean exit stops what Syn started; a killed process runs no cleanup at
all, and that is how people end programs: Stop in the console, the
`Stop-Process` line scripts/console.ps1 prints, Ctrl+C, closing a window.
Each scenario starts something real, kills it with no warning, waits, and
lists what survived. Anything that did is reported, then killed, so a
failing run does not leave this machine worse than it found it.

  1. the CLI, mid-request: its curl must go with it;
  2. the console, mid-turn: its CLI must go, and so the turn stops
     spending tokens for nobody;
  3. the MCP server, with a helper and an office it started: both must go
     (needs LibreOffice and python3-uno; skipped otherwise, and on
     Windows, where the helper drives Office itself);
  4. the MCP server, with Syn's own browser open on a page: every browser
     process on its profile must go (needs Chrome, Edge or Chromium;
     skipped otherwise).

Exit 1 if anything was left. Needs `cli`, `ui` and `mcpgate` built.
"""

import json
import os
import shutil
import subprocess
import sys
import threading
import time
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from devkit import REPO, WINDOWS, Report, alive, binary, children, kill, matching, scratch  # noqa: E402
from fakemodel import FakeModel  # noqa: E402

SLOW = ["word "] * 60  # a minute of streaming: long enough to be killed in


def free_port():
    """A port nothing is on. Not `--port 0`: consoles before it was fixed
    checked requests against port 0 and refused every one, and this tool
    has to run against old builds to show what changed."""
    import socket
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_for(pred, secs):
    until = time.time() + secs
    while time.time() < until:
        if pred():
            return True
        time.sleep(0.2)
    return pred()


def survivors(r, name, needle, grace=4):
    """After `grace` seconds, whatever still carries `needle`: reported, then killed."""
    time.sleep(grace)
    left = matching(needle)
    r.check(name, not left, "; ".join(c[:100] for _, c in left))
    for pid, _ in left:
        kill(pid)


def cli_mid_request(r):
    # Silent while it "thinks": a curl with nothing arriving has nothing to
    # fail on, and would wait out its own timeout. One that is streaming
    # dies of a broken pipe at its next write, leak or no leak.
    with FakeModel(words=SLOW, delay=1.0, think=60) as m:
        p = subprocess.Popen([binary("cli")], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL, text=True, env=dict(os.environ, **m.env(scratch("cli"))))
        p.stdin.write("say hello\n")
        p.stdin.flush()
        if not wait_for(lambda: matching(m.url), 15):
            r.check("CLI killed mid-request leaves no curl", False, "the request never started")
            p.kill()
            return
        p.kill()
        p.wait()
        survivors(r, "CLI killed mid-request leaves no curl", m.url)


def console_mid_turn(r):
    # Silent too, for the CLI's sake this time: one printing a streamed reply
    # to a console that is gone dies of a broken pipe by accident.
    with FakeModel(words=SLOW, delay=1.0, think=60) as m:
        home = scratch("ui")
        url_file = os.path.join(home, "url.txt")
        ui = subprocess.Popen([binary("ui"), "--port", str(free_port()), "--url-file", url_file], stdin=subprocess.DEVNULL,
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=dict(os.environ, **m.env(home)))
        if not wait_for(lambda: os.path.exists(url_file) and open(url_file).read().strip(), 20):
            r.check("console killed mid-turn leaves no CLI", False, "the console never came up")
            ui.kill()
            return
        url = open(url_file).read().split()[0].rstrip("/")

        def say():
            req = urllib.request.Request(url + "/cmd", data=b"say hello", method="POST",
                                         headers={"Origin": url, "Content-Type": "text/plain"})
            try:
                urllib.request.urlopen(req, timeout=120).read()
            except Exception:
                pass  # the console is about to be killed under this request

        threading.Thread(target=say, daemon=True).start()
        if not wait_for(lambda: matching(m.url), 20):
            r.check("console killed mid-turn leaves no CLI", False, "the turn never reached the model")
            ui.kill()
            return
        mine = children(ui.pid)
        ui.kill()
        ui.wait()
        # The console's own children (its CLI), and the curl that CLI was
        # waiting on: both must be gone.
        survivors(r, "console killed mid-turn leaves no curl", m.url)
        left = [pid for pid in mine if alive(pid)]
        r.check("console killed mid-turn leaves no CLI", not left, ("pids " + ", ".join(map(str, left))) if left else "")
        for pid in left:
            kill(pid)


def mcpgate_with_helper(r):
    python = os.environ.get("AGENT_PYTHON") or "/usr/bin/python3"
    if WINDOWS:
        r.skip("MCP server killed with a helper running", "drives Office on Windows; run it by hand there")
        return
    if not shutil.which("soffice") or subprocess.run([python, "-c", "import uno"], capture_output=True).returncode:
        r.skip("MCP server killed with a helper running", "needs LibreOffice and python3-uno")
        return
    home = scratch("mcp")
    docs = os.path.join(home, "docs")
    subprocess.run([python, os.path.join(REPO, "sidecar-lo", "make_fixtures.py"), docs], check=True,
                   capture_output=True, timeout=180)
    pipe = "devcheck%d" % os.getpid()
    env = dict(os.environ, AGENT_ENV_FILE=home + "/no.env", AGENT_HOME=home + "/h", AGENT_PYTHON=python,
               AGENT_PIPE_EXCEL=pipe + "-excel", AGENT_PIPE_WORD=pipe + "-word", AGENT_PIPE_PPT=pipe + "-ppt",
               AGENT_MCP_ROOTS=docs)
    srv = subprocess.Popen([binary("mcpgate")], cwd=REPO, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                           stderr=subprocess.DEVNULL, text=True)

    def rpc(n, method, params):
        srv.stdin.write(json.dumps({"jsonrpc": "2.0", "id": n, "method": method, "params": params}) + "\n")
        srv.stdin.flush()
        return json.loads(srv.stdout.readline())

    rpc(1, "initialize", {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "devcheck", "version": "1"}})
    rpc(2, "tools/call", {"name": "open", "arguments": {"app": "excel", "path": os.path.join(docs, "sales.xlsx")}})
    started = matching(pipe)
    if not started:
        r.check("MCP server killed with a helper leaves nothing", False, "no helper started")
        srv.kill()
        return
    r.note("%d processes started for the helper" % len(started))
    srv.kill()
    srv.wait()
    survivors(r, "MCP server killed with a helper leaves nothing", pipe, grace=10)


def mcpgate_with_browser(r):
    # The browser is the one child that is not Syn's own program and has
    # a dozen processes of its own: a killed Syn that left even the
    # renderers would leave a window on the person's screen, signed in.
    import importlib.util
    spec = importlib.util.spec_from_file_location("syn_fixture_site", os.path.join(REPO, "tests", "browser", "site.py"))
    site = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(site)
    server, base = site.start()
    home = scratch("browser")
    env = dict(os.environ, AGENT_ENV_FILE=home + "/no.env", AGENT_HOME=home + "/h", AGENT_CDP="",
               AGENT_BROWSER_HEADLESS="1", AGENT_MCP_ROOTS=home)
    srv = subprocess.Popen([binary("mcpgate")], cwd=REPO, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                           stderr=subprocess.DEVNULL, text=True)
    try:
        def rpc(n, method, params):
            srv.stdin.write(json.dumps({"jsonrpc": "2.0", "id": n, "method": method, "params": params}) + "\n")
            srv.stdin.flush()
            return json.loads(srv.stdout.readline())

        rpc(1, "initialize", {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "devcheck", "version": "1"}})
        got = rpc(2, "tools/call", {"name": "open", "arguments": {"app": "browser", "path": base + "/form"}})
        said = json.dumps(got)
        if "no Chrome or Edge" in said or "turned off" in said:
            r.skip("MCP server killed with a browser open leaves nothing", "no Chrome, Edge or Chromium here")
            return
        profile = os.path.join(home, "h", "browser-profile")
        started = matching(profile)
        if not started:
            r.check("MCP server killed with a browser open leaves nothing", False, "no browser started: " + said[:200])
            return
        r.note("%d browser processes started" % len(started))
        srv.kill()
        srv.wait()
        survivors(r, "MCP server killed with a browser open leaves nothing", profile, grace=5)
    finally:
        srv.kill()
        server.shutdown()


def main():
    r = Report("what survives when Syn is killed")
    cli_mid_request(r)
    console_mid_turn(r)
    mcpgate_with_helper(r)
    mcpgate_with_browser(r)
    return r.done()


if __name__ == "__main__":
    sys.exit(main())
