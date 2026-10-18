"""Live check that the window follows Syn's work (sidecar-csharp/Host/Reveal.cs), on real Excel, Word and PowerPoint.

Windows with desktop Office only; not part of CI. Build first:
    cd core && cargo build --release --bins && cd .. && dotnet build sidecar-csharp/Host/Host.csproj -c Release
    python tests/dev/reveal_live.py

It drives the real mcpgate and the freshly built office-host against scratch files in a temp folder, and after each step reads
where the application's window is looking. The helper is started on pipe names of its own: mcpgate would otherwise attach to the
helper of a Syn that is running (the installed one), and the build under test would never run. Only what it opened is closed:
the scratch workbook by name, and Word or PowerPoint only when no document is left in them. A workbook of yours stays open.
"""
import json, os, re, shutil, subprocess, sys, tempfile, time

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
HOST = os.environ.get("HOSTEXE") or os.path.join(REPO, "sidecar-csharp", "Host", "bin", "Release", "net8.0-windows", "office-host.exe")
GATE = os.path.join(REPO, "core", "target", "release", "mcpgate.exe")
for exe in (HOST, GATE):
    if not os.path.isfile(exe):
        sys.exit(f"missing {exe}: build first (see this file's header)")

folder = tempfile.mkdtemp(prefix="syn-reveal-")
csv = os.path.join(folder, "reveal.csv")
with open(csv, "w") as f:
    f.write("Month,Revenue\n" + "".join(f"m{i},{i}\n" for i in range(1, 400)))
env = {k: v for k, v in os.environ.items() if not k.startswith("AGENT_")}
env.update(AGENT_ENV_FILE=os.path.join(folder, "no.env"), AGENT_HOME=os.path.join(folder, "home"), AGENT_MCP_ROOTS=folder,
           AGENT_BROWSER="off", AGENT_OFFICE_HOST=HOST, AGENT_PIPE_EXCEL="reveal-live-excel",
           AGENT_PIPE_WORD="reveal-live-word", AGENT_PIPE_PPT="reveal-live-ppt")
srv = subprocess.Popen([GATE], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                       text=True, encoding="utf-8", errors="replace", bufsize=1)
n = 0
H = {}
fails = []


def rpc(method, params):
    global n
    n += 1
    srv.stdin.write(json.dumps({"jsonrpc": "2.0", "id": n, "method": method, "params": params}) + "\n")
    srv.stdin.flush()
    return json.loads(srv.stdout.readline())


def ps(script):
    return subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script],
                          capture_output=True, text=True).stdout.strip()


def call(which, tool, **args):
    args = {k: (v.replace("{H}", H.get(which, "")) if isinstance(v, str) else v) for k, v in args.items()}
    r = rpc("tools/call", {"name": tool, "arguments": args})
    if "error" in r:
        return "RPC ERROR " + json.dumps(r["error"])[:160]
    txt = r["result"]["content"][0]["text"]
    if tool == "open":
        m = re.search(r"Handle: (\S+)", txt)
        H[which] = m.group(1) if m else ""
    return ("ERROR: " if r["result"].get("isError") else "") + txt.replace("\n", " ")[:120]


XL = r"""$o=[Runtime.InteropServices.Marshal]::GetActiveObject('Excel.Application'); $w=$o.ActiveWindow; $v=$w.VisibleRange
$s=try{$o.Selection.Address($false,$false)}catch{'shape:'+$o.Selection.Name}
"$($o.ActiveWorkbook.Name)|$($o.ActiveSheet.Name)|$($w.ScrollRow)|$($w.ScrollColumn)|$($v.Rows.Count)|$($v.Columns.Count)|$s" """
WD = r"""$w=[Runtime.InteropServices.Marshal]::GetActiveObject('Word.Application'); $s=$w.Selection
"$($w.ActiveDocument.Name)|$($w.ActiveDocument.Range(0,$s.Start).Paragraphs.Count)|$($s.Information(3))" """
PP = r"""$p=[Runtime.InteropServices.Marshal]::GetActiveObject('PowerPoint.Application'); "$($p.ActivePresentation.Name)|$($p.ActiveWindow.View.Slide.SlideIndex)" """


def check(name, ok, got):
    print(f"{'PASS' if ok else 'FAIL'}  {name}  ({got})")
    if not ok:
        fails.append(name)


def excel(label, tool, expect, **args):
    call("excel", tool, **args)
    time.sleep(0.6)
    book, sheet, row, col, rows, cols, sel = (ps(XL).split("|") + [""] * 7)[:7]
    check(label, expect(sheet, int(row or 0), int(col or 0), int(rows or 0), int(cols or 0), sel),
          f"sheet {sheet}, top row {row}, left col {col}, {rows}x{cols} visible, selected {sel}")


def word(label, tool, expect, **args):
    call("word", tool, **args)
    time.sleep(0.6)
    doc, para, page = (ps(WD).split("|") + [""] * 3)[:3]
    check(label, expect(int(para or 0), int(page or 0)), f"paragraph {para}, page {page}")


def ppt(label, tool, expect, **args):
    call("ppt", tool, **args)
    time.sleep(0.6)
    deck, slide = (ps(PP).split("|") + [""] * 2)[:2]
    check(label, expect(int(slide or 0)), f"slide {slide}")


rpc("initialize", {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "reveal-live", "version": "1"}})
try:
    print("== Excel")
    call("excel", "open", app="excel", path=csv)
    # top = target row - half the window, give or take two rows
    excel("a write far below scrolls to it and centres it", "write",
          lambda sh, r, c, rows, cols, sel: abs((r + rows // 2) - 200) <= 2 and sel == "A200:B200",
          handle="{H}", selector="reveal!A200:B200", values="far|1")
    excel("a write far to the right scrolls sideways", "write",
          lambda sh, r, c, rows, cols, sel: c <= 27 <= c + cols and abs((c + cols // 2) - 27) <= 2 and sel == "AA10",
          handle="{H}", selector="reveal!AA10", values="right")
    excel("a new sheet comes up", "struct", lambda sh, r, c, rows, cols, sel: sh == "Second",
          handle="{H}", verb="addSheet", name="Second")
    excel("a chart takes the view back to its sheet and is selected", "struct",
          lambda sh, r, c, rows, cols, sel: sh == "reveal" and sel.startswith("shape:") and r <= 40 and r + rows >= 55,
          handle="{H}", verb="chart", kind="column", source="reveal!A1:B4", at="reveal!E40:K55", title="Rev")
    excel("a renamed sheet is the one shown", "struct", lambda sh, r, c, rows, cols, sel: sh == "Notes",
          handle="{H}", verb="sheet", selector="Second", action="rename", name="Notes")
    excel("a table is selected", "struct", lambda sh, r, c, rows, cols, sel: sh == "reveal" and sel == "A1:B4",
          handle="{H}", verb="table", source="reveal!A1:B4", name="Rev1")

    print("== Word")
    call("word", "open", app="word", path=os.path.join(folder, "w.docx"), create=True)
    for i in range(1, 71):
        call("word", "struct", handle="{H}", verb="insertParagraph", text=f"Paragraph {i}: " + "text " * 40)
    word("an appended paragraph is in view", "struct", lambda p, pg: p >= 70 and pg >= 5,
         handle="{H}", verb="insertParagraph", text="LAST PARAGRAPH")
    word("a table at the end is in view", "struct", lambda p, pg: p >= 70,
         handle="{H}", verb="insertTable", rows="a|b;1|2;3|4")
    word("a write far above scrolls back up", "write", lambda p, pg: p == 5 and pg == 1,
         handle="{H}", selector="p5", values="REWRITTEN")
    word("an insert in the middle is shown", "struct", lambda p, pg: p == 40,
         handle="{H}", verb="insertParagraph", at="p40", text="INSERTED")

    print("== PowerPoint")
    call("ppt", "open", app="powerpoint", path=os.path.join(folder, "p.pptx"), create=True)
    for k, t in enumerate(("One", "Two", "Three", "Four"), 1):
        ppt(f"a new slide ({t}) is shown", "struct", lambda s, k=k: s == k, handle="{H}", verb="createSlide", title=t, values="a|b")
    ppt("a change to slide 1 goes back to it", "format", lambda s: s == 1, handle="{H}", selector="s1", style="size=30")
    ppt("a duplicate is shown where it landed", "struct", lambda s: s == 2, handle="{H}", verb="duplicateSlide", selector="s1")
    ppt("a moved slide is shown where it landed", "struct", lambda s: s == 2, handle="{H}", verb="moveSlide", selector="s4", at="s2")
finally:
    for which in ("word", "ppt"):
        if H.get(which):
            call(which, "struct", handle="{H}", verb="save")
            call(which, "struct", handle="{H}", verb="close")
    srv.stdin.close()
    try:
        srv.wait(30)
    except Exception:
        srv.kill()
    # only what this run opened: its scratch workbook by name, and an Office application only once it holds no document
    ps(r"""$o=$null; try { $o=[Runtime.InteropServices.Marshal]::GetActiveObject('Excel.Application'); $o.DisplayAlerts=$false
        foreach ($b in @($o.Workbooks)) { if ($b.Name -eq 'reveal.csv') { $b.Close($false) } }; $o.DisplayAlerts=$true } catch {}""")
    for prog, coll in (("Word.Application", "Documents"), ("PowerPoint.Application", "Presentations")):
        ps(f"""try {{ $o=[Runtime.InteropServices.Marshal]::GetActiveObject('{prog}'); if ($o.{coll}.Count -eq 0) {{ $o.Quit() }} }} catch {{}}""")
    shutil.rmtree(folder, ignore_errors=True)

print("\n" + ("all checks passed" if not fails else f"{len(fails)} check(s) FAILED: {fails}"))
sys.exit(1 if fails else 0)
