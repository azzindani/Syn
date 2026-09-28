"""What the dev tools share: where the binaries are, which processes are
running, and a report that ends in an exit code.

Standard library only, like the rest of the Python here.
"""

import os
import subprocess
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
WINDOWS = sys.platform == "win32"


def binary(name):
    """A built binary: SYN_BIN_DIR if set, else core/target/{debug,release}."""
    exe = name + (".exe" if WINDOWS else "")
    dirs = [os.environ["SYN_BIN_DIR"]] if os.environ.get("SYN_BIN_DIR") else [
        os.path.join(REPO, "core", "target", b) for b in ("debug", "release")
    ]
    for d in dirs:
        p = os.path.join(d, exe)
        if os.path.exists(p):
            return p
    sys.exit("%s is not built: cd core && cargo build --bins (or set SYN_BIN_DIR)" % exe)


def scratch(prefix):
    """A fresh folder for one run's AGENT_HOME and files."""
    return tempfile.mkdtemp(prefix="syn-dev-%s-" % prefix)


def processes():
    """Every process as (pid, command line)."""
    if WINDOWS:
        # tasklist has no command lines; CIM does.
        out = subprocess.run(
            ["powershell", "-NoProfile", "-Command",
             "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId) $($_.CommandLine)\" }"],
            capture_output=True, text=True).stdout
    else:
        out = subprocess.run(["ps", "-eo", "pid=,args="], capture_output=True, text=True).stdout
    rows = []
    for line in out.splitlines():
        pid, _, cmd = line.strip().partition(" ")
        if pid.isdigit() and int(pid) != os.getpid():
            rows.append((int(pid), cmd.strip()))
    return rows


def matching(*needles):
    """Processes whose command line holds every one of `needles`."""
    return [(p, c) for p, c in processes() if all(n in c for n in needles)]


def children(pid):
    """The processes `pid` started."""
    if WINDOWS:
        out = subprocess.run(
            ["powershell", "-NoProfile", "-Command",
             "Get-CimInstance Win32_Process -Filter \"ParentProcessId=%d\" | ForEach-Object { $_.ProcessId }" % pid],
            capture_output=True, text=True).stdout
    else:
        out = subprocess.run(["pgrep", "-P", str(pid)], capture_output=True, text=True).stdout
    return [int(x) for x in out.split() if x.isdigit()]


def alive(pid):
    return any(p == pid for p, _ in processes())


def kill(pid):
    try:
        if WINDOWS:
            subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"], capture_output=True)
        else:
            os.kill(pid, 9)
    except OSError:
        pass


class Report:
    """Checks, printed as they finish, and an exit code at the end."""

    def __init__(self, title):
        self.title = title
        self.failed = []
        print("== %s" % title)

    def check(self, name, ok, detail=""):
        print("  %s %s%s" % ("PASS" if ok else "FAIL", name, (": " + detail) if detail else ""))
        if not ok:
            self.failed.append(name)
        return ok

    def skip(self, name, why):
        print("  SKIP %s: %s" % (name, why))

    def note(self, text):
        print("       %s" % text)

    def done(self):
        if self.failed:
            print("== %s: %d failed (%s)" % (self.title, len(self.failed), ", ".join(self.failed)))
            return 1
        print("== %s: all passed" % self.title)
        return 0
