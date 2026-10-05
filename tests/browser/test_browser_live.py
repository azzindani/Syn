"""Syn's own browser, against a real Chromium: an MCP client talks to the real
`mcpgate`, which starts a browser of its own and drives the fixture site in
tests/browser/site.py.

Nothing here sets a debugging port or an address: the browser is found, started
on a profile of its own, driven and ended by Syn, which is the point. (The
older tests/test_cdp_live.py is the other mode, a browser the person started
and pointed Syn at.)

What this proves, page by page: an address opens in a tab and comes back with a
map of what can be pressed or filled; a form is filled and sent with real
input, and the page it lands on is said; a password or card field is refused;
a button something covers is refused with what covers it; a button that wants a
real mouse gets one; an alert is closed and a question answered Cancel, each
reported; frames and shadow roots are entered; a page that fills in late is
waited for; a menu that opens on hover is opened; pressing Next three times is
not a loop; a link that opens a tab says so; back, forward, reload; addresses a
page must never be sent to are refused; text that is hidden is not read and
text that is shown comes back fenced; long text is read in pieces; a sign-in
survives Syn and the browser ending; the browser goes when Syn goes and
comes back when it is wanted.

Skips without a Chromium (AGENT_BROWSER, SYN_BROWSER, PW_CHROMIUM, or Chrome or
Edge or Chromium installed) or a built `mcpgate`, unless SYN_REQUIRE_BROWSER=1
(CI), where a missing piece is a failure. The browser runs with no window unless
SYN_BROWSER_VISIBLE=1, which is how to watch it.
"""

import importlib.util
import json
import os
import queue
import re
import shutil
import subprocess
import tempfile
import threading
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
EXE = ".exe" if os.name == "nt" else ""


def load_site():
    # `site.py` is also a standard library module's name: load it by path.
    spec = importlib.util.spec_from_file_location("syn_fixture_site", os.path.join(HERE, "site.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def browser_exe():
    for p in (os.environ.get("AGENT_BROWSER"), os.environ.get("SYN_BROWSER"), os.environ.get("PW_CHROMIUM"),
              r"C:\Program Files\Google\Chrome\Application\chrome.exe",
              r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
              r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
              "/opt/pw-browsers/chromium", "/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"):
        if p and p.lower() != "off" and os.path.isfile(p):
            return p
    for name in ("chromium", "chromium-browser", "google-chrome", "google-chrome-stable", "microsoft-edge"):
        found = shutil.which(name)
        if found:
            return found
    return None


def mcpgate():
    for b in ("debug", "release"):
        p = os.path.join(REPO, "core", "target", b, "mcpgate" + EXE)
        if os.path.exists(p):
            return p
    return None


def missing():
    if not browser_exe():
        return "no Chromium, Chrome or Edge found (set AGENT_BROWSER)"
    if not mcpgate():
        return "mcpgate is not built (cd core && cargo build --bins)"
    return None


REASON = missing()
if REASON and os.environ.get("SYN_REQUIRE_BROWSER") == "1":
    raise RuntimeError("SYN_REQUIRE_BROWSER=1 but " + REASON)


def browser_processes(token):
    """How many browser processes have this folder on their command line."""
    if os.name == "nt":
        cmd = ("@(Get-CimInstance Win32_Process | Where-Object { $_.Name -match '^(chrome|msedge)\\.exe$' -and "
               "$_.CommandLine -like '*%s*' }).Count" % token)
        out = subprocess.run(["powershell", "-NoProfile", "-Command", cmd], capture_output=True, text=True).stdout.strip()
        return int(out or 0)
    out = subprocess.run(["ps", "-eo", "args"], capture_output=True, text=True).stdout
    return sum(1 for line in out.splitlines() if token in line and "ps -eo" not in line)


def kill_browser_processes(token):
    if os.name == "nt":
        cmd = ("Get-CimInstance Win32_Process | Where-Object { $_.Name -match '^(chrome|msedge)\\.exe$' -and "
               "$_.CommandLine -like '*%s*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }" % token)
        subprocess.run(["powershell", "-NoProfile", "-Command", cmd], capture_output=True)
    else:
        subprocess.run(["pkill", "-9", "-f", token], capture_output=True)


def wait_until(fn, seconds=15, step=0.25):
    end = time.time() + seconds
    while time.time() < end:
        value = fn()
        if value:
            return value
        time.sleep(step)
    return fn()


class Gate:
    """One `mcpgate`, spoken to over stdio, with a watchdog on every read."""

    def __init__(self, env):
        self.proc = subprocess.Popen([mcpgate()], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.DEVNULL, text=True, bufsize=1)
        self.lines = queue.Queue()
        threading.Thread(target=self._pump, daemon=True).start()
        self.n = 0
        self.rpc("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                "clientInfo": {"name": "browser-live-test", "version": "1"}})

    def _pump(self):
        for line in self.proc.stdout:
            self.lines.put(line)
        self.lines.put(None)

    def rpc(self, method, params, wait=120):
        self.n += 1
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}) + "\n")
        self.proc.stdin.flush()
        try:
            line = self.lines.get(timeout=wait)
        except queue.Empty:
            raise AssertionError("mcpgate did not answer %s within %d s: a call hung" % (method, wait))
        if line is None:
            raise AssertionError("mcpgate ended while waiting for %s" % method)
        return json.loads(line)

    def call(self, tool, **args):
        r = self.rpc("tools/call", {"name": tool, "arguments": args})
        if "error" in r:
            return True, json.dumps(r["error"])
        r = r["result"]
        return r.get("isError", False), r["content"][0]["text"]

    def stop(self):
        if self.proc.poll() is None:
            try:
                self.proc.stdin.close()
            except OSError:
                pass
            try:
                self.proc.wait(30)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(10)
        for pipe in (self.proc.stdin, self.proc.stdout):
            try:
                pipe.close()
            except OSError:
                pass


@unittest.skipIf(REASON, REASON or "")
class SynsOwnBrowser(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.site_mod = load_site()
        cls.http, cls.base = cls.site_mod.start()
        cls.dir = tempfile.mkdtemp(prefix="syn-browser-live-")
        cls.home = os.path.join(cls.dir, "home")
        cls.out = os.path.join(cls.dir, "out")
        os.makedirs(cls.out)
        cls.token = os.path.basename(cls.dir)
        cls.gate = cls.start_gate()
        cls.h = None

    @classmethod
    def env(cls, **extra):
        env = {k: v for k, v in os.environ.items() if not k.startswith("AGENT_")}
        env.update(AGENT_ENV_FILE=os.path.join(cls.dir, "no.env"), AGENT_HOME=cls.home, AGENT_MCP_ROOTS=cls.out)
        if os.environ.get("SYN_BROWSER_VISIBLE") != "1":
            env["AGENT_BROWSER_HEADLESS"] = "1"
        exe = browser_exe()
        if exe:
            env["AGENT_BROWSER"] = exe
        env.update(extra)
        return env

    @classmethod
    def start_gate(cls, **extra):
        return Gate(cls.env(**extra))

    @classmethod
    def tearDownClass(cls):
        cls.gate.stop()
        # Syn's own shutdown is what is under test; this is only for a failure
        # that left something behind.
        wait_until(lambda: browser_processes(cls.token) == 0, 10)
        kill_browser_processes(cls.token)
        cls.http.shutdown()
        shutil.rmtree(cls.dir, ignore_errors=True)

    # ---- helpers -------------------------------------------------------

    def call(self, tool, **args):
        return self.gate.call(tool, **args)

    def ok(self, tool, **args):
        err, text = self.call(tool, **args)
        self.assertFalse(err, "%s %s failed: %s" % (tool, args, text))
        return text

    def refused(self, tool, **args):
        err, text = self.call(tool, **args)
        self.assertTrue(err, "%s %s should have been refused but said: %s" % (tool, args, text))
        return text

    def verb(self, verb, handle=None, **args):
        return self.ok("struct", handle=handle or self.h, verb=verb, **args)

    def verb_refused(self, verb, handle=None, **args):
        return self.refused("struct", handle=handle or self.h, verb=verb, **args)

    def click(self, selector, handle=None):
        return self.verb("invoke", handle, selector=selector, action="click")

    def read(self, selector, handle=None):
        return self.ok("read", handle=handle or self.h, selector=selector)

    def page(self, path):
        """Go to a page of the fixture in the tab Syn opened, opening it first."""
        url = self.base + path
        if not self.h:
            text = self.ok("open", app="browser", path=url)
            type(self).h = re.search(r"Handle: (\S+)", text).group(1)
            return text
        return self.verb("goto", text=url)

    # ---- the tests, in the order a session meets them -------------------

    def test_01_an_address_starts_syns_own_browser_and_comes_back_with_a_map_of_the_page(self):
        text = self.page("/form")
        self.assertRegex(self.h, r"^web:tab-[0-9a-z]{5}::doc$")
        self.assertIn("opened Syn's browser", text, "the first page says a browser of Syn's own was started")
        self.assertIn("PAGE \"Contact form\"", text)
        self.assertIn("FIELDS", text)
        self.assertIn("#name", text)
        self.assertIn("button \"Send\"", text)
        self.assertIn("a password or card field", text, "the person's fields are marked as theirs")
        port_file = os.path.join(self.home, "browser-profile", "DevToolsActivePort")
        self.assertTrue(os.path.exists(port_file), "the browser listens on a port it chose and wrote down")
        self.assertGreater(browser_processes(self.token), 0)

    def test_02_a_form_is_filled_and_sent_with_real_input_and_the_landing_page_is_said(self):
        self.page("/form")
        self.ok("write", handle=self.h, selector="label=Your name", values="Ada Lovelace")
        said = self.verb("type", selector="#email", text="ada@example.test")
        self.assertIn('it now holds "ada@example.test"', said, "a dot and an at sign are typed as themselves")
        self.assertIn("Green", self.verb("choose", selector="#color", text="green"))
        self.assertIn("It is now checked.", self.click("label=I agree"))
        self.verb("type", selector="#msg", text="Line one\nLine two")
        landed = self.click("text=Send")
        self.assertIn('The page is now "Thanks" at ' + self.base + "/submit", landed)
        self.assertIn('read{"handle":"%s","selector":":map"}' % self.h, landed, "a call to copy, with the real handle")
        body = self.read("body")
        # The page shows the textarea on one line (its markup collapses the
        # break), so the two lines being there at all is the Enter key typed.
        for want in ("Thanks, Ada Lovelace", "email=ada@example.test", "color=green", "agree=on", "msg=Line one Line two"):
            self.assertIn(want, body)

    def test_03_what_the_person_types_is_refused_to_read_write_and_type(self):
        self.page("/form")
        for selector in ("#pw", "#card"):
            self.assertIn("Syn does not", self.refused("write", handle=self.h, selector=selector, values="x"))
            self.assertIn("Syn does not type", self.verb_refused("type", selector=selector, text="x"))
        self.assertIn("is never read", self.read("#pw"))
        self.assertEqual(self.read("#pw").count("hunter2"), 0)

    def test_04_a_covered_button_says_what_covers_it_and_works_once_that_is_gone(self):
        self.page("/overlay")
        why = self.refused("struct", handle=self.h, verb="invoke", selector="#covered", action="click")
        self.assertIn("is covered by", why)
        self.assertIn("veil", why)
        self.click("#dismiss")
        self.click("#covered")
        self.assertIn("covered clicked", self.read("#out"))

    def test_05_a_button_that_wants_a_real_mouse_gets_one(self):
        self.page("/overlay")
        self.click("#dismiss")
        self.click("#needs-real")
        out = self.read("#out")
        self.assertIn("trusted click", out)
        self.assertNotIn("untrusted", out)

    def test_06_an_alert_is_closed_a_question_is_answered_cancel_and_both_are_said(self):
        self.page("/dialogs")
        said = self.click("#do-alert")
        self.assertIn('a alert dialog appeared and said "hello from alert"; Syn closed it', said)
        self.assertIn("alert closed", self.read("#out"))
        said = self.click("#do-confirm")
        self.assertIn('asked "really?"', said)
        self.assertIn("answered Cancel", said)
        self.assertIn("confirm:false", self.read("#out"))
        self.click("#do-prompt")
        self.assertIn("prompt:null", self.read("#out"))
        # A page that asks before it is left is left.
        self.verb("type", selector="#dirty", text="unsaved")
        left = self.click("#leave")
        self.assertIn("/form", left)

    def test_07_frames_and_shadow_roots_are_entered_with_three_arrows(self):
        self.page("/frames")
        self.assertIn("inner-btn", self.read(":map"), "the map lists what is inside a frame")
        self.click("#inner >>> #inner-btn")
        self.assertIn("inner clicked", self.read("#inner >>> #inner-out"))
        self.click("#outer-btn")
        self.assertIn("outer clicked", self.read("#outer-out"))
        self.page("/shadow")
        self.assertIn("shadow-btn", self.read(":map"))
        self.click("#card >>> #shadow-btn")
        self.assertIn("shadow clicked", self.read("#card >>> #shadow-out"))

    def test_08_a_page_that_fills_in_late_is_waited_for_not_polled(self):
        self.page("/spa")
        waited = self.verb("wait", selector="#items")
        self.assertIn("is on the page", waited)
        self.assertIn("3 items", self.read("#count"))
        self.click("#more")
        self.verb("wait", text="6 items")
        self.assertIn("6 items", self.read("#count"))
        late = self.verb_refused("wait", text="never appears", name="1")
        self.assertIn("still waiting for the words \"never appears\" to appear after 1 s", late)

    def test_09_a_menu_that_opens_on_hover_is_opened_and_pressed(self):
        self.page("/hover")
        # The pointer stays where the last test left it, as a real one does,
        # and the page may have loaded with it over the menu. Move it off.
        self.verb("hover", selector="h1")
        self.assertIn("not visible", self.refused("struct", handle=self.h, verb="invoke", selector="#sub-item", action="click"))
        self.verb("hover", selector="#menu-host")
        went = self.click("#sub-item")
        self.assertIn("/form", went)

    def test_10_pressing_next_three_times_is_progress_not_a_loop_and_a_missing_next_is_explained(self):
        self.page("/table?page=1")
        for want in ("page=2", "page=3", "page=4"):
            self.assertIn(want, self.click("#next"), "three identical presses in a row, each on a new page")
        self.assertIn("Item 31", self.read("table"))
        gone = self.refused("struct", handle=self.h, verb="invoke", selector="#next", action="click")
        self.assertIn("no element matches", gone)
        self.assertIn(":map", gone, "the way out is named")

    def test_11_a_link_that_opens_a_tab_says_so_and_the_tab_can_be_read_and_closed(self):
        self.page("/newtab")
        said = self.click("#blank")
        m = re.search(r"a new tab opened \(.*?\): (tab-[0-9a-z]{5})", said)
        self.assertIsNotNone(m, said)
        alias = m.group(1)
        opened = self.ok("open", app="browser", path=alias)
        handle = re.search(r"Handle: (\S+)", opened).group(1)
        self.assertEqual(handle, "web:%s::doc" % alias)
        self.assertIn("Contact form", self.read("h1", handle=handle))
        self.assertIn("closed %s" % alias, self.verb("close", handle=handle))
        gone = self.refused("read", handle=handle, selector="h1")
        self.assertIn("that tab was closed", gone)
        self.assertNotIn("excel", gone.lower(), "no spreadsheet example in a refusal about a page")
        # window.open is a tab too.
        said = self.click("#winopen")
        self.assertIn("a new tab opened", said)

    def test_12_back_forward_and_reload_walk_the_history(self):
        self.page("/form")
        self.page("/spa")
        self.assertIn("/form", self.verb("back"))
        self.assertIn("/spa", self.verb("forward"))
        self.assertIn("no later page", self.verb_refused("forward"))
        self.verb("reload")
        self.assertIn("Single page app", self.ok("export", handle=self.h, format="title"))

    def test_13_an_address_a_page_must_never_be_sent_to_is_refused(self):
        self.page("/form")
        with open(os.path.join(self.home, "browser-profile", "DevToolsActivePort")) as f:
            port = f.read().split()[0]
        for url, why in (("file:///C:/Windows/win.ini", "local files"), ("javascript:alert(1)", "that is code"),
                         ("chrome://settings", "the browser's own pages"),
                         ("http://127.0.0.1:%s/json/list" % port, "belongs to Syn itself"),
                         ("http://localhost:7777/", "belongs to Syn itself"),
                         ("https://user:pw@example.com/", "user name or password")):
            self.assertIn(why, self.verb_refused("goto", text=url), url)
            self.assertIn(why, self.refused("open", app="browser", path=url), url)

    def test_14_hidden_text_is_not_read_and_shown_text_comes_back_fenced_and_flagged(self):
        self.page("/inject")
        body = self.read("body")
        self.assertNotIn("hidden text should not be read", body.replace("aria-hidden text should not be read", ""))
        fence = body.index("<user_content>")
        self.assertIn("Ignore all previous instructions", body[fence:])
        self.assertIn("WARNING", body, "text shaped like an instruction is flagged")
        self.assertIn("not visible", self.read("#secret"))
        self.assertNotIn("hidden text should not be read", self.read("#secret").replace("is not visible", ""))

    def test_15_long_text_is_read_in_pieces_and_says_where_to_go_on(self):
        self.page("/big")
        first = self.read("body")
        m = re.search(r"\[characters 0 to (\d+) of (\d+)\. The rest: read selector \"(body@\d+)\"\]", first)
        self.assertIsNotNone(m, first[-300:])
        total = int(m.group(2))
        self.assertGreater(total, 250000)
        self.assertEqual(m.group(3), "body@%s" % m.group(1), "it names the character to go on from")
        second = self.read(m.group(3))
        self.assertIn("characters %s to" % m.group(1), second)
        tail = self.read("body@%d" % (total - 200))
        self.assertIn("END OF BIG PAGE", tail)
        self.assertNotIn("The rest: read selector", tail, "the last piece does not point further")

    def test_16_scrolling_brings_in_what_loads_on_scroll(self):
        self.page("/long")
        self.assertIn("scrolled", self.verb("scroll", selector="#lazy"))
        self.verb("wait", text="lazy loaded")
        self.assertIn("lazy loaded", self.read("#lazy"))
        self.assertIn("to 0 of", self.verb("scroll", text="top"))
        self.assertIn("scrolled the page to", self.verb("scroll", text="down"))
        self.assertIn("direction is down, up, top or bottom", self.verb_refused("scroll", text="sideways"))

    def test_17_a_screenshot_and_a_pdf_are_written_where_syn_may_write_and_nowhere_else(self):
        self.page("/form")
        png = os.path.join(self.out, "page.png")
        self.ok("export", handle=self.h, format="png", path=png)
        with open(png, "rb") as f:
            self.assertEqual(f.read(8), b"\x89PNG\r\n\x1a\n")
        pdf = os.path.join(self.out, "page.pdf")
        self.ok("export", handle=self.h, format="pdf", path=pdf)
        with open(pdf, "rb") as f:
            self.assertEqual(f.read(4), b"%PDF")
        elsewhere = os.path.join(tempfile.gettempdir(), "syn-browser-live-outside.png")
        self.assertFalse(os.path.exists(elsewhere))
        self.refused("export", handle=self.h, format="png", path=elsewhere)
        self.assertFalse(os.path.exists(elsewhere))
        self.assertIn("Contact form", self.ok("export", handle=self.h, format="text"))
        self.assertIn("FIELDS", self.ok("export", handle=self.h, format="summary"))

    def test_18_keys_reach_the_page_rich_editors_and_controlled_inputs_keep_what_is_typed(self):
        self.page("/editor")
        self.verb("type", selector="#rich", text="Hello rich")
        self.assertIn("Hello rich", self.read("#rich"))
        self.ok("write", handle=self.h, selector="#ctl", values="controlled")
        self.assertIn("controlled", self.read("#mirror"))
        # Suggestions appear only for real key events.
        self.verb("type", selector="#auto", text="ap")
        sugg = self.read("#sugg")
        self.assertIn("apple", sugg)
        self.assertIn("apricot", sugg)
        self.verb("press", selector="#enter", text="Enter")
        self.assertIn("enter pressed", self.read("#out"))
        self.verb("press", selector="#ctl", text="Control+a")

    def test_19_find_says_where_words_are_written_and_a_words_selector_that_fits_many_is_refused(self):
        self.page("/table?page=1")
        found = self.verb("find", text="Item 7")
        self.assertIn("place(s) on the page say \"Item 7\"", found)
        refused = self.refused("struct", handle=self.h, verb="invoke", selector="text=Item", action="click")
        self.assertRegex(refused, r"\d+ elements match text=Item\. Use one of these selectors:")
        self.assertIn("td:nth-of-type", refused)

    def test_20_a_page_that_cannot_be_reached_or_answers_with_an_error_says_so_in_words(self):
        self.page("/form")
        import socket
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            dead = s.getsockname()[1]
        self.assertIn("nothing is listening at that address",
                      self.verb_refused("goto", text="http://127.0.0.1:%d/" % dead))
        self.assertIn("that address does not exist",
                      self.verb_refused("goto", text="http://no-such-host.invalid/"))
        landed = self.verb("goto", text=self.base + "/missing")
        self.assertIn("HTTP 404", landed)
        self.assertIn("probably not the page that was wanted", landed)
        self.assertIn("HTTP 500", self.verb("goto", text=self.base + "/server-error"))

    def test_21_a_slow_page_is_waited_for_and_a_redirect_is_followed(self):
        self.page("/form")
        began = time.time()
        said = self.verb("goto", text=self.base + "/slow")
        self.assertGreaterEqual(time.time() - began, 2.5, "the call returned only when the page had loaded")
        self.assertIn("Slow page", said)
        self.assertIn("Slow done", self.read("h1"))
        self.assertIn("/form", self.verb("goto", text=self.base + "/redirect"))

    def test_22_a_title_that_changes_does_not_lose_the_tab(self):
        self.page("/title-change")
        self.click("#rename")
        self.assertIn("After", self.ok("export", handle=self.h, format="title"))
        # The handle was never the title: it still reads and presses.
        self.assertTrue(self.read("body").strip())
        self.click("#go")
        self.assertIn("Contact form", self.ok("export", handle=self.h, format="title"))

    def test_23_open_finds_a_tab_by_words_and_refuses_words_no_tab_has_listing_the_tabs(self):
        self.page("/spa")
        found = self.ok("open", app="browser", path="Single page")
        self.assertIn("Handle: " + self.h, found)
        why = self.refused("open", app="browser", path="no such tab anywhere")
        self.assertIn("no browser tab has", why)
        self.assertIn("tab-", why, "the tabs there are are named by what to use")
        self.assertIn("open{", why, "and how to open an address is a call to copy")

    def test_24_zz_sign_ins_are_kept_the_browser_goes_with_syn_and_comes_back_when_wanted(self):
        self.page("/cookie")
        self.assertIn("first visit", self.read("#state"))
        # End Syn the polite way. The browser it started goes with it,
        # and a site's cookie, written on the way out, is kept.
        self.gate.stop()
        self.assertTrue(wait_until(lambda: browser_processes(self.token) == 0, 20), "the browser outlived Syn")
        type(self).gate = self.start_gate()
        type(self).h = None
        self.page("/cookie")
        self.assertIn("welcome back", self.read("#state"), "the profile kept the cookie across the browser ending")

    def test_25_zz_a_browser_the_person_closed_starts_again_with_the_next_open(self):
        self.page("/form")
        kill_browser_processes(self.token)
        self.assertTrue(wait_until(lambda: browser_processes(self.token) == 0, 15))
        why = self.refused("read", handle=self.h, selector="h1")
        self.assertIn("browser window was closed", why)
        self.assertIn("open", why)
        type(self).h = None
        text = self.page("/spa")
        self.assertIn("opened Syn's browser", text, "started again, with the same profile")
        self.assertIn("Single page app", self.ok("export", handle=self.h, format="title"))

    def test_26_zzz_the_browser_can_be_turned_off(self):
        self.gate.stop()
        wait_until(lambda: browser_processes(self.token) == 0, 20)
        off = self.start_gate(AGENT_BROWSER="off")
        try:
            err, text = off.call("open", app="browser", path=self.base + "/form")
            self.assertTrue(err, text)
            self.assertIn("turned off", text)
        finally:
            off.stop()
        type(self).gate = self.start_gate()
        type(self).h = None


if __name__ == "__main__":
    unittest.main()
