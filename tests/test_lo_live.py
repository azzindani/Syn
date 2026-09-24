"""The whole stack against a real office engine, on Linux: an MCP client
talks to the real `mcpgate`, which starts `sidecar-lo/lo_host.py`, which
starts LibreOffice, which opens real .xlsx/.docx/.pptx files, evaluates the
formulas and writes the results back out.

What this proves: the MCP protocol as a client speaks it, the desk (connect,
launch, open, bind, first look), the gates, the `office-rpc/1` wire, the
guidance a model reads, and that the documents end up right -- checked by
unzipping the exported files, not by asking Syn. What it cannot prove is
anything about Microsoft Office or the C# helper; that is tier 3, on
Windows (docs/10, docs/11).

Skips without LibreOffice + python3-uno or a built `mcpgate`, unless
SYN_REQUIRE_LO=1 (CI's libreoffice job), where a missing piece is a failure.
"""

import json
import os
import re
import shutil
import subprocess
import tempfile
import time
import unittest
import zipfile

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PYTHON = os.environ.get("AGENT_PYTHON") or "/usr/bin/python3"


def missing():
    if not shutil.which("soffice"):
        return "LibreOffice (soffice) is not installed"
    if subprocess.run([PYTHON, "-c", "import uno"], capture_output=True).returncode != 0:
        return "%s cannot import uno (python3-uno)" % PYTHON
    for b in ("release", "debug"):
        exe = os.path.join(REPO, "core", "target", b, "mcpgate")
        if os.path.exists(exe):
            return None
    return "mcpgate is not built (cd core && cargo build --bins)"


def mcpgate_exe():
    for b in ("debug", "release"):
        exe = os.path.join(REPO, "core", "target", b, "mcpgate")
        if os.path.exists(exe):
            return exe


REASON = missing()
if REASON and os.environ.get("SYN_REQUIRE_LO") == "1":
    raise RuntimeError("SYN_REQUIRE_LO=1 but " + REASON)


# The fixture's own rule (sidecar-lo/make_fixtures.py), so the expected
# numbers come from the data's definition, never from Syn or LibreOffice.
REGIONS = ["North", "South", "East", "West"]
UNITS = {r: 0 for r in REGIONS}
REVENUE = {r: 0.0 for r in REGIONS}
for _i in range(6):
    for _j, _r in enumerate(REGIONS):
        _u = 40 + (_i * 7 + _j * 13) % 60
        UNITS[_r] += _u
        REVENUE[_r] += _u * (12.5 + _j)


@unittest.skipIf(REASON, REASON or "")
class LiveAgainstLibreOffice(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.mkdtemp(prefix="syn-lo-live-")
        cls.docs = os.path.join(cls.dir, "docs")
        cls.out = os.path.join(cls.dir, "out")
        subprocess.run([PYTHON, os.path.join(REPO, "sidecar-lo", "make_fixtures.py"), cls.docs],
                       check=True, capture_output=True, timeout=180)
        pipe = "t%d" % os.getpid()
        env = dict(os.environ,
                   AGENT_ENV_FILE=os.path.join(cls.dir, "no.env"),
                   AGENT_HOME=os.path.join(cls.dir, "home"),
                   AGENT_PIPE_EXCEL=pipe + "-excel", AGENT_PIPE_WORD=pipe + "-word", AGENT_PIPE_PPT=pipe + "-ppt",
                   AGENT_PYTHON=PYTHON,
                   AGENT_MCP_ROOTS=cls.docs + ";" + cls.out)
        for k in ("AGENT_MCP_APPS", "AGENT_MCP_LAUNCH", "AGENT_OFFICE_HOST"):
            env.pop(k, None)
        cls.srv = subprocess.Popen([mcpgate_exe()], cwd=REPO, env=env, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
        cls.n = 0
        cls.rpc("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                               "clientInfo": {"name": "lo-live-test", "version": "1"}})

    @classmethod
    def tearDownClass(cls):
        cls.srv.stdin.close()
        cls.srv.wait(60)
        cls.srv.stdout.close()
        shutil.rmtree(cls.dir, ignore_errors=True)

    @classmethod
    def rpc(cls, method, params):
        cls.n += 1
        cls.srv.stdin.write(json.dumps({"jsonrpc": "2.0", "id": cls.n, "method": method, "params": params}) + "\n")
        cls.srv.stdin.flush()
        line = cls.srv.stdout.readline()
        return json.loads(line)

    def call(self, tool, **args):
        r = self.rpc("tools/call", {"name": tool, "arguments": args})["result"]
        return r.get("isError", False), r["content"][0]["text"]

    def ok(self, tool, **args):
        err, text = self.call(tool, **args)
        self.assertFalse(err, "%s %s failed: %s" % (tool, args, text))
        return text

    def value(self, text):
        return re.search(r"= (.*)\n</user_content>", text).group(1)

    def test_a_workbook_is_opened_computed_and_saved(self):
        opened = self.ok("open", app="excel", path=os.path.join(self.docs, "sales.xlsx"))
        self.assertIn("sheets: data, Q3 sales", opened)
        # The suggested next call, run exactly as written.
        suggested = json.loads(re.search(r"Next: read(\{.*?\})", opened).group(1))
        self.assertIn("grid data: 20x8", self.ok("read", **suggested))

        h = "excel:sales.xlsx:workbook"
        self.ok("write", handle=h, selector="data!E1", values="Revenue")
        self.assertIn("filled data!E2:E25 (24 cells)", self.ok("write", handle=h, selector="data!E2:E25", values="=C2*D2"))
        self.assertEqual(self.value(self.ok("read", handle=h, selector="data!E2:E3")), "500;715.5")

        # The shapes a small model sends: no prefix, rows as arrays.
        self.ok("struct", handle="sales.xlsx", verb="addSheet", name="Summary")
        rows = [["Region", "Units", "Revenue"]] + [
            [r, "=SUMIF(data!$A$2:$A$25,A%d,data!$C$2:$C$25)" % (k + 2), "=SUMIF(data!$A$2:$A$25,A%d,data!$E$2:$E$25)" % (k + 2)]
            for k, r in enumerate(REGIONS)]
        self.ok("write", handle="sales.xlsx", selector="Summary!A1:C5", values=rows)
        self.ok("write", handle=h, selector="Summary!A6:C6", values="Total|=SUM(B2:B5)|=SUM(C2:C5)")
        self.ok("struct", handle=h, verb="chart", kind="column", source="Summary!A1:C5", at="Summary!E1:L16",
                title="By region")
        self.ok("format", handle=h, selector="Summary!A1:C1", style="bold=1;fill=#1F4E79;color=#FFFFFF")

        # A quoted sheet name, as the guidance tells models to write one.
        self.assertIn("Bearspaw", self.ok("read", handle=h, selector="'Q3 sales'!A1:B3"))

        self.ok("export", handle=h, format="xlsx", path=os.path.join(self.out, "sales.xlsx"))
        z = zipfile.ZipFile(os.path.join(self.out, "sales.xlsx"))
        names = re.findall(r'<sheet name="([^"]+)"', z.read("xl/workbook.xml").decode())
        sx = z.read("xl/worksheets/sheet%d.xml" % (names.index("Summary") + 1)).decode()

        def cell(ref):
            return re.search(r'<c r="%s"[^>]*>(?:<f[^>]*>([^<]*)</f>)?<v>([^<]*)</v>' % ref, sx).groups()

        for k, r in enumerate(REGIONS):
            f, v = cell("C%d" % (k + 2))
            self.assertTrue(f.startswith("SUMIF("), "a live formula, not a pasted number: %s" % f)
            self.assertAlmostEqual(float(v), REVENUE[r])
            self.assertEqual(int(float(cell("B%d" % (k + 2))[1])), UNITS[r])
        self.assertAlmostEqual(float(cell("C6")[1]), sum(REVENUE.values()))
        self.assertTrue(any(n.startswith("xl/charts/") for n in z.namelist()), "the chart is in the file")

    def test_a_memo_and_a_deck_are_written(self):
        self.ok("open", app="word", path=os.path.join(self.docs, "memo.docx"))
        w = "word:memo.docx:body"
        self.assertIn("paras=4", self.ok("read", handle=w, selector="body"))
        self.assertIn("Site visit memo", self.ok("read", handle=w, selector="p0"), "p0 is the first paragraph")
        self.ok("struct", handle=w, verb="insertParagraph", name="Heading 1", text="Findings")
        self.ok("struct", handle=w, verb="insertTable", rows=[["Region", "Revenue"], ["East", "6,394.50"]])
        self.ok("export", handle=w, format="docx", path=os.path.join(self.out, "memo.docx"))
        doc = zipfile.ZipFile(os.path.join(self.out, "memo.docx")).read("word/document.xml").decode()
        self.assertIn("Findings", doc)
        self.assertIn("<w:tbl>", doc)

        self.ok("open", app="powerpoint", path=os.path.join(self.docs, "deck.pptx"))
        p = "ppt:deck.pptx:deck"
        self.ok("struct", handle=p, verb="createSlide", title="East leads", bullets=["East first", ">West close"])
        self.ok("write", handle=p, selector="s1.notes", values="Lead with East.")
        self.assertIn("s1: East leads", self.ok("read", handle=p, selector="deck"))
        self.ok("export", handle=p, format="pptx", path=os.path.join(self.out, "deck.pptx"))
        z = zipfile.ZipFile(os.path.join(self.out, "deck.pptx"))
        self.assertIn("East leads", z.read("ppt/slides/slide1.xml").decode())
        self.assertTrue(any("Lead with East" in z.read(n).decode() for n in z.namelist() if n.startswith("ppt/notesSlides/")))

    def test_mistakes_come_back_with_the_way_to_fix_them(self):
        self.ok("open", app="excel", path=os.path.join(self.docs, "sales.xlsx"))
        h = "excel:sales.xlsx:workbook"
        err, text = self.call("read", handle=h, selector="A1:C5")
        self.assertTrue(err)
        self.assertIn("this workbook has data, Q3 sales", text)
        err, text = self.call("read", handle=h, sheet="data", range="A1")
        self.assertTrue(err)
        self.assertIn("The fields are: handle, selector", text)
        err, text = self.call("open", app="word", path=os.path.join(self.docs, "sales.xlsx"))
        self.assertTrue(err)
        self.assertIn("an Excel file", text)
        err, text = self.call("open", app="excel", path="/etc/other.xlsx")
        self.assertTrue(err)
        self.assertIn("outside the folders", text)
        # The application's own refusal, with what it means after it.
        err, text = self.call("open", app="excel", path=os.path.join(self.docs, "nope.xlsx"))
        self.assertTrue(err)
        self.assertIn("no such file", text)
        self.assertIn("What to do: No file exists at that path", text)


@unittest.skipIf(REASON, REASON or "")
class NothingIsLeftRunning(unittest.TestCase):
    def test_closing_the_client_stops_the_helpers_and_their_offices(self):
        d = tempfile.mkdtemp(prefix="syn-lo-stop-")
        try:
            subprocess.run([PYTHON, os.path.join(REPO, "sidecar-lo", "make_fixtures.py"), d],
                           check=True, capture_output=True, timeout=180)
            env = dict(os.environ, AGENT_ENV_FILE=os.path.join(d, "no.env"), AGENT_HOME=os.path.join(d, "home"),
                       AGENT_PIPE_EXCEL="stop%d-excel" % os.getpid(), AGENT_PYTHON=PYTHON)
            srv = subprocess.Popen([mcpgate_exe()], cwd=REPO, env=env, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
            srv.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                "name": "open", "arguments": {"app": "excel", "path": os.path.join(d, "sales.xlsx")}}}) + "\n")
            srv.stdin.flush()
            self.assertIn("Opened in Excel", srv.stdout.readline())
            helper = subprocess.run(["pgrep", "-f", "lo_host.py --pipe stop%d-excel" % os.getpid()],
                                    capture_output=True, text=True).stdout.split()
            self.assertTrue(helper, "the helper was started")
            srv.stdin.close()
            self.assertEqual(srv.wait(60), 0)
            srv.stdout.close()
            time.sleep(1)
            for pid in helper:
                self.assertFalse(os.path.exists("/proc/%s" % pid), "helper %s outlived the server" % pid)
            left = subprocess.run(["pgrep", "-f", "syn-lo-excel-"], capture_output=True, text=True).stdout.split()
            self.assertEqual(left, [], "LibreOffice outlived its helper")
        finally:
            shutil.rmtree(d, ignore_errors=True)


class Gate:
    """One MCP server over stdio, for tests that need more than one."""

    def __init__(self, env, name):
        self.p = subprocess.Popen([mcpgate_exe()], cwd=REPO, env=env, stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
        self.n = 0
        self.rpc("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                "clientInfo": {"name": name, "version": "1"}})

    def rpc(self, method, params):
        self.n += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}) + "\n")
        self.p.stdin.flush()
        return json.loads(self.p.stdout.readline())

    def call(self, tool, **args):
        r = self.rpc("tools/call", {"name": tool, "arguments": args})["result"]
        return r.get("isError", False), r["content"][0]["text"]

    def close(self):
        self.p.stdin.close()
        self.p.wait(60)
        self.p.stdout.close()


@unittest.skipIf(REASON, REASON or "")
class TwoSessionsAtOnce(unittest.TestCase):
    """Two MCP clients on one machine -- a desktop app and the console, or
    two desktop apps -- driving the same Excel, the way a person would have
    them. A helper used to serve one client at a time, and the second one's
    `open` hung until the first went away."""

    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.mkdtemp(prefix="syn-lo-two-")
        cls.docs = os.path.join(cls.dir, "docs")
        subprocess.run([PYTHON, os.path.join(REPO, "sidecar-lo", "make_fixtures.py"), cls.docs],
                       check=True, capture_output=True, timeout=180)
        cls.pipe = "two%d" % os.getpid()
        cls.env = dict(os.environ, AGENT_ENV_FILE=os.path.join(cls.dir, "no.env"),
                       AGENT_HOME=os.path.join(cls.dir, "home"), AGENT_PYTHON=PYTHON,
                       AGENT_PIPE_EXCEL=cls.pipe + "-excel", AGENT_PIPE_WORD=cls.pipe + "-word",
                       AGENT_PIPE_PPT=cls.pipe + "-ppt", AGENT_MCP_ROOTS=cls.docs)
        for k in ("AGENT_MCP_APPS", "AGENT_MCP_LAUNCH", "AGENT_OFFICE_HOST"):
            cls.env.pop(k, None)
        cls.a = Gate(cls.env, "session-a")
        cls.b = Gate(cls.env, "session-b")

    @classmethod
    def tearDownClass(cls):
        cls.a.close()
        cls.b.close()
        shutil.rmtree(cls.dir, ignore_errors=True)

    def ok(self, gate, tool, **args):
        err, text = gate.call(tool, **args)
        self.assertFalse(err, "%s %s failed: %s" % (tool, args, text))
        return text

    def test_a_both_sessions_drive_one_workbook_and_see_each_other(self):
        wb = os.path.join(self.docs, "sales.xlsx")
        self.ok(self.a, "open", app="excel", path=wb)
        started = time.time()
        opened = self.ok(self.b, "open", app="excel", path=wb)
        self.assertLess(time.time() - started, 10, "the second session waited on the first")
        self.assertIn("already open", opened)
        h = "excel:sales.xlsx:workbook"
        self.ok(self.b, "write", handle=h, selector="data!F1", values="from b")
        self.assertIn("from b", self.ok(self.a, "read", handle=h, selector="data!F1"))
        self.ok(self.a, "write", handle=h, selector="data!F2", values="from a")
        self.assertIn("from a", self.ok(self.b, "read", handle=h, selector="data!F2"))

    def test_b_one_session_holds_three_apps_while_the_other_works(self):
        # Excel, Word and PowerPoint in one session, one helper each, while
        # the other session keeps using Excel.
        self.ok(self.a, "open", app="word", path=os.path.join(self.docs, "memo.docx"))
        self.ok(self.a, "open", app="powerpoint", path=os.path.join(self.docs, "deck.pptx"))
        self.assertIn("Site visit memo", self.ok(self.a, "read", handle="word:memo.docx:body", selector="p0"))
        self.ok(self.a, "struct", handle="ppt:deck.pptx:deck", verb="createSlide", title="Shared", bullets=["one", "two"])
        self.assertIn("Shared", self.ok(self.a, "read", handle="ppt:deck.pptx:deck", selector="s1"))
        self.assertIn("Region", self.ok(self.b, "read", handle="excel:sales.xlsx:workbook", selector="data!A1"))

    def test_c_a_helper_that_dies_is_recovered_by_opening_again(self):
        # Killed outright, so nothing of it gets to clean up: its LibreOffice
        # is left running with the workbook open. The session must say how
        # to recover, recover on `open`, and not refuse the retry as a loop.
        h = "excel:sales.xlsx:workbook"
        self.ok(self.a, "read", handle=h, selector="data!A1:B2")
        pids = subprocess.run(["pgrep", "-f", "lo_host.py --pipe %s-excel" % self.pipe],
                              capture_output=True, text=True).stdout.split()
        self.assertTrue(pids)
        for pid in pids:
            os.kill(int(pid), 9)
        time.sleep(1)
        err, text = self.a.call("read", handle=h, selector="data!A1:B2")
        self.assertTrue(err)
        self.assertIn("open", text)
        err, text = self.a.call("read", handle=h, selector="data!A1:B2")
        self.assertTrue(err)
        self.assertIn("Call `open`", text, "a paused session must say how to mend it")
        self.assertIn("already open", self.ok(self.a, "open", app="excel", path=os.path.join(self.docs, "sales.xlsx")))
        self.assertIn("Region", self.ok(self.a, "read", handle=h, selector="data!A1:B2"))
        # The other session reconnects the same way.
        self.ok(self.b, "open", app="excel", path=os.path.join(self.docs, "sales.xlsx"))
        self.assertIn("Region", self.ok(self.b, "read", handle=h, selector="data!A1"))


if __name__ == "__main__":
    unittest.main()
