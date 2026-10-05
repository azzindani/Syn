"""Close Syn's window, and see whether what the browser was holding was kept.

    python3 tests/dev/window_close.py          # Windows, in a desktop session

Closing the window ends Syn. It used to end on the spot, and the job object
that ties Syn's children to it killed the CLI and the browser the CLI had
started in the same instant, so the browser never wrote out a sign-in made in
the last half minute: the person was signed out again at the next start. A
test that ends Syn by closing its stdin cannot see that (the polite path
works); only closing the window does.

  1. the real window opens on a copy of the console, the CLI and the window
     program in a folder of their own, with no keys and no .env;
  2. a page sets a cookie that is to last a year;
  3. the window is closed the way a person closes it;
  4. every process of Syn and of its browser must be gone, and the console
     must have let go of its port at once;
  5. a console started again must find the cookie.

Needs `cli`, `ui` built, and the window program built
(dotnet build -c Release sidecar-csharp/Window), a browser (Chrome or Edge),
and a desktop: it opens a window for a few seconds. Skipped elsewhere.
"""

import importlib.util
import os
import re
import shutil
import socket
import subprocess
import sys
import time
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from devkit import REPO, WINDOWS, Report, binary, matching, scratch  # noqa: E402

WINDOW_DIR = os.path.join(REPO, "sidecar-csharp", "Window", "bin", "Release", "net8.0-windows")


def load_site():
    spec = importlib.util.spec_from_file_location("syn_fixture_site", os.path.join(REPO, "tests", "browser", "site.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def listening(port):
    with socket.socket() as s:
        s.settimeout(0.5)
        return s.connect_ex(("127.0.0.1", port)) == 0


def wait_for(pred, secs):
    until = time.time() + secs
    while time.time() < until:
        if pred():
            return True
        time.sleep(0.1)
    return pred()


def close_window(folder):
    """Ask the window program's window to close, as the close button does."""
    ps = ("Get-Process syn-window -ErrorAction SilentlyContinue | "
          "Where-Object { $_.Path -like '%s*' } | ForEach-Object { [void]$_.CloseMainWindow() }" % folder.replace("'", "''"))
    subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True)


def main():
    r = Report("what closing the window keeps")
    if not WINDOWS:
        r.skip("closing the window keeps a sign-in", "the window is a Windows program")
        return r.done()
    host = os.path.join(WINDOW_DIR, "syn-window.exe")
    if not os.path.exists(host):
        r.skip("closing the window keeps a sign-in", "build the window first: dotnet build -c Release sidecar-csharp/Window")
        return r.done()

    folder = scratch("window")
    for name in ("ui", "cli"):
        shutil.copy(binary(name), folder)
    # The whole build output: the WebView2 loader sits in a subfolder, and
    # without it the window program starts, fails and hands over to a browser.
    shutil.copytree(WINDOW_DIR, folder, dirs_exist_ok=True)
    ui = os.path.join(folder, "ui.exe")
    url_file = os.path.join(folder, "url.txt")
    port = free_port()
    env = {k: v for k, v in os.environ.items() if not k.startswith("AGENT_")}
    env.update(AGENT_HOME=os.path.join(folder, ".agent"), AGENT_BROWSER_HEADLESS="1", AGENT_ENV_FILE=os.path.join(folder, "none.env"))
    # The profile Syn's own browser keeps its sign-ins in, which the test looks for in command lines.
    profile = os.path.join(folder, ".agent", "browser-profile")

    server, base = load_site().start()

    def start(window):
        if os.path.exists(url_file):
            os.remove(url_file)
        args = [ui, "--port", str(port), "--url-file", url_file] + (["--open"] if window else [])
        p = subprocess.Popen(args, cwd=folder, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)
        if not wait_for(lambda: os.path.exists(url_file) and open(url_file).read().strip(), 20):
            return p, None
        return p, open(url_file).read().split()[0].rstrip("/")

    def cmd(url, text):
        req = urllib.request.Request(url + "/cmd", data=text.encode(), method="POST",
                                     headers={"Origin": url, "Content-Type": "text/plain"})
        return urllib.request.urlopen(req, timeout=90).read().decode("utf-8", "replace")

    def state(url):
        out = cmd(url, "open browser %s/cookie" % base)
        handle = re.search(r"open (web:\S+)", out)
        if not handle:
            return "no handle: " + out[:200]
        return cmd(url, "read %s #state" % handle.group(1))

    try:
        p, url = start(window=True)
        if not url:
            r.check("the console starts", False, "no address after 20 s")
            return r.done()
        if not wait_for(lambda: bool(matching("syn-window", folder)), 20):
            r.skip("closing the window keeps a sign-in", "no window appeared (no desktop session, or no WebView2)")
            return r.done()
        first = state(url)
        r.check("the page says it is a first visit", "first visit" in first, "" if "first visit" in first else first[:120])
        started = len(matching(profile))
        r.note("%d browser processes while the window is open" % started)
        t0 = time.time()
        close_window(folder)
        gone = wait_for(lambda: p.poll() is not None, 40)
        r.check("closing the window ends Syn", gone, "" if gone else "the console is still running 40 s after the window closed")
        r.note("the console ended %.1fs after the window closed" % (time.time() - t0))
        r.check("no browser or Syn process is left", wait_for(lambda: not matching(profile) and not matching(folder), 20),
                "; ".join(c[:90] for _, c in (matching(profile) + matching(folder))[:3]))
        r.check("the console let go of its port", not listening(port))
        p2, url2 = start(window=False)
        if not url2:
            r.check("the console starts again", False, "no address after 20 s")
            return r.done()
        second = state(url2)
        kept = "welcome back" in second
        r.check("a console started again finds what the browser was holding", kept,
                "" if kept else "the cookie set before the window closed was lost: " + second[:120])
        p2.kill()
        p2.wait()
        wait_for(lambda: not matching(profile), 10)
    finally:
        server.shutdown()
        for pid, _ in matching(folder) + matching(profile):
            subprocess.run(["taskkill", "/F", "/T", "/PID", str(pid)], capture_output=True)
        shutil.rmtree(folder, ignore_errors=True)
    return r.done()


if __name__ == "__main__":
    sys.exit(main())
