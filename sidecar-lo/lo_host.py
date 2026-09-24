#!/usr/bin/env python3
"""lo_host.py -- the office helper for machines without Microsoft Office.

Speaks `office-rpc/1` exactly as `sidecar-csharp/Host` (office-host.exe)
does -- one JSON line in, one JSON line out, the same method names, the same
replies word for word where it can -- but drives LibreOffice through UNO
instead of Office through COM, and listens on a Unix socket instead of a
Windows named pipe.

Why it exists: every line of Syn above the helper -- the MCP server, the
desk, the gates, the hand transport, the wire format, the guidance a model
reads -- can then be tested against a real office engine that evaluates
formulas and writes real .xlsx/.docx/.pptx files, on Linux, in CI. What it
cannot test is the C# itself: COM quirks, Excel's own behaviour, the
single-instance rules in docs/runbook-windows.md. A green run here is
evidence about everything except the Windows helper.

Run:  python3 lo_host.py --pipe hand-excel --app excel [--trace]
The socket is $XDG_RUNTIME_DIR/syn-pipe-<name>.sock, else
/tmp/syn-pipe-<name>.sock -- `core::hand::pipe_path` computes the same.

One helper per app, like office-host. Each starts its own LibreOffice with
its own throwaway profile (two instances sharing a profile lock each other
out), headless unless AGENT_LO_VISIBLE=1 and a display is present. When the
helper stops -- SIGTERM, or its parent dying -- its LibreOffice goes with it.
Nothing is saved on the way out: `export` writes a copy, as it does for
Office, and a headless instance has no human to save for.
"""

import argparse
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time

import uno  # noqa: E402  (LibreOffice's Python bridge: python3-uno)
from com.sun.star.beans import PropertyValue  # noqa: E402

READ_CELL_CAP = 200  # office-host.exe's ReadCellCap
TRACE = False


def trace(msg):
    if TRACE:
        sys.stderr.write(time.strftime("[%H:%M:%S] ") + msg + "\n")
        sys.stderr.flush()


def socket_path(name):
    base = os.environ.get("XDG_RUNTIME_DIR") or "/tmp"
    return os.path.join(base, "syn-pipe-%s.sock" % name)


class Refused(Exception):
    """A request the helper understood and will not or cannot do."""


def reply_ok(preview):
    # CR/LF become spaces, as office-host's Esc does: a reply is one line.
    return json.dumps({"ok": True, "preview": preview.replace("\r", " ").replace("\n", " ")},
                      ensure_ascii=False, separators=(",", ":"))


def reply_fail(error):
    return json.dumps({"ok": False, "error": error.replace("\r", " ").replace("\n", " ")},
                      ensure_ascii=False, separators=(",", ":"))


def prop(name, value):
    p = PropertyValue()
    p.Name = name
    p.Value = value
    return p


def trunc(s, n=120):
    s = s or ""
    return s.strip() if len(s) <= n else s[:n].strip() + "..."


def escape_cell(s):
    return s.replace("\\", "\\\\").replace("|", "\\|").replace(";", "\\;")


def parse_grid(payload):
    """Cells by |, rows by ;, a backslash escaping the next character --
    office-host's ParseGrid and core's grid(), the same rule."""
    rows = [[""]]
    escaped = False
    for ch in payload or "":
        row = rows[-1]
        if escaped:
            row[-1] += ch
            escaped = False
        elif ch == "\\":
            escaped = True
        elif ch == ";":
            rows.append([""])
        elif ch == "|":
            row.append("")
        else:
            row[-1] += ch
    return rows


def split_range(selector):
    """Sheet!A1:B2 -> ("Sheet", "A1:B2"). Quotes around a sheet name are
    Excel's own syntax for a name with a space, 'Q3 sales'!A1, and are
    taken off; a doubled quote inside is one quote."""
    i = selector.find("!")
    sheet, addr = (selector, "") if i < 0 else (selector[:i], selector[i + 1:])
    if len(sheet) >= 2 and sheet[0] == "'" and sheet[-1] == "'":
        sheet = sheet[1:-1].replace("''", "'")
    return sheet, addr


def col_name(c):
    s = ""
    c += 1
    while c:
        c, r = divmod(c - 1, 26)
        s = chr(65 + r) + s
    return s


def truthy(v):
    return v in ("1", "true", "True", "yes", "on")


def rgb(v):
    h = v.lstrip("#")
    if len(h) != 6:
        raise Refused("colour %s is not a six-digit hex like #1F4E79" % v)
    try:
        return int(h, 16)  # LibreOffice colours are 0xRRGGBB already
    except ValueError:
        raise Refused("colour %s is not a six-digit hex like #1F4E79" % v)


class Office:
    """One LibreOffice instance, started and owned by this helper."""

    def __init__(self, app, visible):
        self.app = app
        self.profile = tempfile.mkdtemp(prefix="syn-lo-%s-" % app)
        self.pipe = "synlo_%s_%d" % (app, os.getpid())
        args = [shutil.which("soffice") or "/usr/lib/libreoffice/program/soffice",
                "--norestore", "--nologo", "--nodefault", "--nolockcheck",
                "-env:UserInstallation=" + uno.systemPathToFileUrl(self.profile),
                "--accept=pipe,name=%s;urp;" % self.pipe]
        self.visible = visible
        if not visible:
            args[1:1] = ["--headless", "--invisible"]
        # Its own session, so stopping it takes the whole tree: `soffice`
        # is a script that starts oosplash that starts soffice.bin.
        self.proc = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL, start_new_session=True)
        local = uno.getComponentContext()
        resolver = local.ServiceManager.createInstanceWithContext("com.sun.star.bridge.UnoUrlResolver", local)
        deadline = time.time() + 90
        while True:
            try:
                self.ctx = resolver.resolve("uno:pipe,name=%s;urp;StarOffice.ComponentContext" % self.pipe)
                break
            except Exception:
                if self.proc.poll() is not None:
                    raise RuntimeError("LibreOffice exited (%s) before it was ready" % self.proc.returncode)
                if time.time() > deadline:
                    raise RuntimeError("LibreOffice did not answer on its pipe within 90s")
                time.sleep(0.3)
        self.desktop = self.ctx.ServiceManager.createInstanceWithContext("com.sun.star.frame.Desktop", self.ctx)
        trace("LibreOffice ready on %s" % self.pipe)

    def stop(self):
        try:
            os.killpg(self.proc.pid, signal.SIGTERM)
            self.proc.wait(10)
        except Exception:
            try:
                os.killpg(self.proc.pid, signal.SIGKILL)
            except Exception:
                pass
        shutil.rmtree(self.profile, ignore_errors=True)

    # ---- documents ------------------------------------------------------

    SERVICE = {
        "excel": "com.sun.star.sheet.SpreadsheetDocument",
        "word": "com.sun.star.text.TextDocument",
        "powerpoint": "com.sun.star.presentation.PresentationDocument",
    }

    def docs(self):
        out = []
        comps = self.desktop.getComponents().createEnumeration()
        while comps.hasMoreElements():
            d = comps.nextElement()
            try:
                if d.supportsService(self.SERVICE[self.app]):
                    out.append(d)
            except Exception:
                pass
        return out

    @staticmethod
    def name_of(doc):
        url = doc.getURL() or ""
        return uno.fileUrlToSystemPath(url).rsplit("/", 1)[-1] if url else ""

    def find(self, handle):
        # office-host's rule: the document whose file name the handle contains.
        for d in self.docs():
            n = self.name_of(d)
            if n and n in handle:
                return d
        return None

    def open(self, path):
        if not path or not path.strip():
            raise Refused("open needs a path")
        if not os.path.isfile(path):
            raise Refused("no such file: %s" % path)
        full = os.path.abspath(path)
        name = os.path.basename(full)
        for d in self.docs():
            if self.name_of(d) == name:
                return "already open: %s" % name
        doc = self.desktop.loadComponentFromURL(uno.systemPathToFileUrl(full), "_blank", 0,
                                                (prop("Hidden", not self.visible),))
        if doc is None or not doc.supportsService(self.SERVICE[self.app]):
            if doc is not None:
                doc.close(True)
            raise Refused("%s did not open as a %s document" % (name, self.app))
        noun = {"excel": "workbook(s)", "word": "document(s)", "powerpoint": "presentation(s)"}[self.app]
        return "opened %s, %d %s" % (name, len(self.docs()), noun)


# ---------------------------------------------------------------- Calc

class Calc:
    def __init__(self, doc):
        self.doc = doc
        mapper = doc.createInstance("com.sun.star.sheet.FormulaOpCodeMapper")
        self.parser = doc.createInstance("com.sun.star.sheet.FormulaParser")
        # Excel's own formula language: English names, comma separators,
        # A1 references. A model writes =SUMIF(A:A,"x",B:B); LibreOffice's
        # native grammar would want semicolons.
        self.parser.OpCodeMap = mapper.getAvailableMappings(
            uno.getConstantByName("com.sun.star.sheet.FormulaLanguage.XL_ENGLISH"),
            uno.getConstantByName("com.sun.star.sheet.FormulaMapGroup.ALL_EXCEPT_SPECIAL"))
        self.parser.CompileEnglish = True
        self.parser.FormulaConvention = uno.getConstantByName("com.sun.star.sheet.AddressConvention.XL_A1")

    def sheet(self, name):
        sheets = self.doc.Sheets
        if sheets.hasByName(name):
            return sheets.getByName(name)
        raise Refused("no sheet named '%s': a selector is Sheet!A1:B2, and this workbook has %s"
                      % (name, ", ".join(sheets.ElementNames)))

    @staticmethod
    def whole(addr):
        """A:C and 2:5 as LibreOffice ranges."""
        a = addr.upper()
        if ":" in a:
            l, r = a.split(":", 1)
            if l.isalpha() and r.isalpha():
                return "%s1:%s1048576" % (l, r)
            if l.isdigit() and r.isdigit():
                return "A%s:XFD%s" % (l, r)
        return addr

    def rng(self, ws, addr):
        try:
            return ws.getCellRangeByName(self.whole(addr))
        except Exception:
            raise Refused("%s is not a range: use A1 or A1:D10" % addr)

    def set_cell(self, cell, v):
        if v.startswith("="):
            cell.setTokens(self.parser.parseFormula(v, cell.getCellAddress()))
            return
        # A number typed into Excel is a number, "1,234" included; anything
        # else is text. Excel's rule, applied the same way here.
        try:
            cell.setValue(float(v.strip().replace(",", "")))
        except ValueError:
            cell.setString(v)

    def read(self, selector):
        sheet, addr = split_range(selector)
        ws = self.sheet(sheet)
        if addr:
            r = self.rng(ws, addr)
        else:
            r = ws.createCursor()
            r.gotoStartOfUsedArea(False)
            r.gotoEndOfUsedArea(True)
        a = r.getRangeAddress()
        rows, cols = a.EndRow - a.StartRow + 1, a.EndColumn - a.StartColumn + 1
        if rows * cols > READ_CELL_CAP:
            return "grid %s: %dx%d (over the %d-cell read cap: narrow the selector to see values)" % (
                sheet, rows, cols, READ_CELL_CAP)
        body = ";".join("|".join(escape_cell(r.getCellByPosition(j, i).getString()) for j in range(cols))
                        for i in range(rows))
        return "grid %s: %dx%d = %s" % (sheet, rows, cols, body)

    def write(self, selector, payload):
        sheet, addr = split_range(selector)
        if not addr:
            raise Refused("write needs Sheet!A1:B2")
        ws = self.sheet(sheet)
        rows = parse_grid(payload)
        target = self.rng(ws, addr)
        a = target.getRangeAddress()
        span = (a.EndRow - a.StartRow + 1) * (a.EndColumn - a.StartColumn + 1)
        # One value into many cells means fill, not "write to the corner",
        # exactly as office-host does it -- and the references step per row.
        if span > 1 and len(rows) == 1 and len(rows[0]) == 1:
            one = rows[0][0]
            self.set_cell(target.getCellByPosition(0, 0), one)
            from com.sun.star.sheet.FillDirection import TO_BOTTOM, TO_RIGHT
            if a.EndRow > a.StartRow:
                ws.getCellRangeByPosition(a.StartColumn, a.StartRow, a.StartColumn, a.EndRow).fillAuto(TO_BOTTOM, 1)
            if a.EndColumn > a.StartColumn:
                target.fillAuto(TO_RIGHT, 1)
            return "filled %s!%s (%s cells) from %s" % (sheet, addr, format(span, ","), trunc(one))
        for i, row in enumerate(rows):
            for j, v in enumerate(row):
                self.set_cell(ws.getCellByPosition(a.StartColumn + j, a.StartRow + i), v)
        wide = max(len(r) for r in rows)
        start = "%s%d" % (col_name(a.StartColumn), a.StartRow + 1)
        end = "%s%d" % (col_name(a.StartColumn + wide - 1), a.StartRow + len(rows))
        return "wrote %d row(s) x %d column(s) into %s!%s:%s" % (len(rows), wide, sheet, start, end)

    def format(self, selector, style):
        sheet, addr = split_range(selector)
        if not addr:
            raise Refused("format needs Sheet!A1:B2")
        ws = self.sheet(sheet)
        r = self.rng(ws, addr)
        applied = []
        for pair in (style or "").split(";"):
            if "=" not in pair:
                continue
            k, v = pair.split("=", 1)
            k, v = k.strip().lower(), v.strip()
            if k == "bold":
                r.CharWeight = 150.0 if truthy(v) else 100.0
            elif k == "italic":
                r.CharPosture = uno.Enum("com.sun.star.awt.FontSlant", "ITALIC" if truthy(v) else "NONE")
            elif k == "size":
                r.CharHeight = float(v)
            elif k in ("numberformat", "format"):
                fmts = self.doc.NumberFormats
                loc = uno.createUnoStruct("com.sun.star.lang.Locale")
                key = fmts.queryKey(v, loc, False)
                if key == -1:
                    key = fmts.addNew(v, loc)
                r.NumberFormat = key
                k = "numberFormat"
            elif k == "width":
                # Excel measures columns in characters; LibreOffice in
                # hundredths of a millimetre. One character is about 1.9mm.
                r.Columns.Width = int(float(v) * 190)
            elif k == "autofit":
                r.Columns.OptimalWidth = True
            elif k == "autofitsheet":
                cur = ws.createCursor()
                cur.gotoEndOfUsedArea(False)
                ws.getCellRangeByPosition(0, 0, cur.getRangeAddress().EndColumn, 0).Columns.OptimalWidth = True
                k = "autofitSheet"
            elif k == "wrap":
                r.IsTextWrapped = truthy(v)
            elif k == "font":
                r.CharFontName = v
            elif k == "color":
                r.CharColor = rgb(v)
            elif k == "fill":
                r.CellBackColor = rgb(v)
            elif k == "merge":
                r.merge(truthy(v))
            elif k == "border":
                line = uno.createUnoStruct("com.sun.star.table.BorderLine2")
                line.OuterLineWidth = 26 if truthy(v) else 0
                tb = r.TableBorder2
                for side in ("TopLine", "BottomLine", "LeftLine", "RightLine", "HorizontalLine", "VerticalLine"):
                    setattr(tb, side, line)
                    setattr(tb, "Is" + side + "Valid", True)
                r.TableBorder2 = tb
            elif k == "align":
                m = {"left": "LEFT", "center": "CENTER", "centre": "CENTER", "right": "RIGHT"}.get(v.lower())
                if not m:
                    raise Refused("align does not know %s" % v)
                r.HoriJustify = uno.Enum("com.sun.star.table.CellHoriJustify", m)
            elif k == "freeze":
                a = r.getRangeAddress()
                ctl = self.doc.getCurrentController()
                ctl.setActiveSheet(ws)
                ctl.freezeAtPosition(a.StartColumn, a.StartRow) if truthy(v) else ctl.freezeAtPosition(0, 0)
            else:
                raise Refused("format does not know %s: it takes bold, italic, size, numberFormat, width, "
                              "autofit, autofitSheet, wrap, font, color, fill, merge, border, align, freeze" % k)
            applied.append(k)
        if not applied:
            raise Refused("format was given no style: try bold=1;numberFormat=#,##0")
        return "formatted %s!%s: %s" % (sheet, addr, ", ".join(applied))

    def add_sheet(self, name):
        if not name or not name.strip():
            raise Refused("addSheet needs a name")
        sheets = self.doc.Sheets
        for n in sheets.ElementNames:
            if n.lower() == name.lower():
                return "sheet %s already exists" % name
        sheets.insertNewByName(name, sheets.getCount())
        return "added sheet %s (%d now)" % (name, sheets.getCount())

    def chart(self, kind, source, title, at, style):
        src_sheet, src_addr = split_range(source)
        if not src_addr:
            raise Refused("chart needs a source like Summary!A1:B12")
        dst_sheet, dst_addr = split_range(at)
        if not dst_addr:
            raise Refused("chart needs a destination like Dashboard!A1")
        diagram = {"line": ("LineDiagram", None), "column": ("BarDiagram", False),
                   "bar": ("BarDiagram", True), "pie": ("PieDiagram", None)}.get((kind or "").lower())
        if not diagram:
            raise Refused("chart does not know %s: it draws line, bar, column or pie" % kind)
        src = self.rng(self.sheet(src_sheet), src_addr).getRangeAddress()
        dst_ws = self.sheet(dst_sheet)
        anchor = self.rng(dst_ws, dst_addr)
        rect = uno.createUnoStruct("com.sun.star.awt.Rectangle")
        rect.X, rect.Y = anchor.Position.X, anchor.Position.Y
        a = anchor.getRangeAddress()
        if a.EndRow > a.StartRow or a.EndColumn > a.StartColumn:
            rect.Width, rect.Height = anchor.Size.Width, anchor.Size.Height
        else:
            rect.Width, rect.Height = 16000, 9000
        charts = dst_ws.Charts
        name = "Chart %d" % (charts.getCount() + 1)
        charts.addNewByName(name, rect, (src,), True, True)
        cd = charts.getByName(name).getEmbeddedObject()
        d = cd.createInstance("com.sun.star.chart." + diagram[0])
        cd.setDiagram(d)
        if diagram[1] is not None:
            cd.getDiagram().Vertical = diagram[1]
        if title:
            cd.HasMainTitle = True
            cd.getTitle().String = title
        for pair in (style or "").split(";"):
            if "=" not in pair:
                continue
            k, v = (x.strip() for x in pair.split("=", 1))
            dg = cd.getDiagram()
            if k == "legend":
                cd.HasLegend = truthy(v)
            elif k == "gridlines":
                dg.HasYAxisGrid = truthy(v)
            elif k == "xTitle":
                dg.HasXAxisTitle = True
                dg.XAxisTitle.String = v
            elif k == "yTitle":
                dg.HasYAxisTitle = True
                dg.YAxisTitle.String = v
            elif k == "dataLabels":
                dg.DataCaption = 1 if truthy(v) else 0
            else:
                raise Refused("chart style does not know %s: it takes legend, gridlines, xTitle, yTitle, dataLabels" % k)
        return "%s chart %s on %s!%s from %s" % (kind, repr(title) if title else "(untitled)", dst_sheet, dst_addr, source)


# ---------------------------------------------------------------- Writer

class Writer:
    def __init__(self, doc):
        self.doc = doc

    def paras(self):
        out = []
        e = self.doc.Text.createEnumeration()
        while e.hasMoreElements():
            p = e.nextElement()
            if p.supportsService("com.sun.star.text.Paragraph"):
                out.append(p)
        return out

    def para(self, n):
        ps = self.paras()
        if n < 0 or n >= len(ps):
            raise Refused("no paragraph p%d: the document has %d (p0 to p%d)" % (n, len(ps), len(ps) - 1))
        return ps[n]

    def style_note(self, target, style):
        if not style or not style.strip():
            return ""
        # Word's names where LibreOffice's differ.
        name = {"Quote": "Quotations", "Normal": "Default Paragraph Style"}.get(style, style)
        fam = self.doc.StyleFamilies.getByName("ParagraphStyles")
        if not fam.hasByName(name):
            return " [style %s is not in this document, left as-is]" % style
        target.ParaStyleName = name
        return " [%s]" % style

    def _new_para(self, text):
        t = self.doc.Text
        cur = t.createTextCursorByRange(t.getEnd())
        from com.sun.star.text.ControlCharacter import PARAGRAPH_BREAK
        t.insertControlCharacter(cur, PARAGRAPH_BREAK, False)
        if text:
            t.insertString(cur, text, False)
        return cur

    def insert_paragraph(self, text, style):
        cur = self._new_para(text or "")
        note = self.style_note(cur, style)
        return "paragraph %d added, %d chars%s" % (len(self.paras()), len(text or ""), note)

    def insert_table(self, grid, style):
        rows = parse_grid(grid)
        if not rows or rows == [[""]]:
            raise Refused("insertTable needs rows: cells by |, rows by ;")
        nc = max(len(r) for r in rows)
        cur = self._new_para("")
        table = self.doc.createInstance("com.sun.star.text.TextTable")
        table.initialize(len(rows), nc)
        self.doc.Text.insertTextContent(cur, table, False)
        for i, row in enumerate(rows):
            for j in range(nc):
                table.getCellByPosition(j, i).setString(row[j] if j < len(row) else "")
        # Word's table styles ("Grid Table 4 - Accent 1") have no LibreOffice
        # equivalent, so a named one is reported rather than faked.
        note = "" if not style else " [table style %s is Word's; left as-is]" % style
        return "table %d added, %dx%d%s" % (self.doc.TextTables.getCount(), len(rows), nc, note)

    def page_break(self, kind):
        cur = self._new_para("")
        cur.BreakType = uno.Enum("com.sun.star.style.BreakType", "PAGE_BEFORE")
        return "%s break added" % (kind or "page")


# ---------------------------------------------------------------- Impress

# office-host's layout names, onto Impress's AutoLayout numbers.
LAYOUTS = {"": 1, "titlecontent": 1, "content": 1, "title": 0, "titleslide": 0, "section": 0,
           "sectionheader": 0, "two": 3, "twocontent": 3, "comparison": 3, "titleonly": 19, "blank": 20}


class Impress:
    def __init__(self, doc):
        self.doc = doc

    def pages(self):
        return self.doc.DrawPages

    @staticmethod
    def shape_of(page, kind):
        for i in range(page.getCount()):
            s = page.getByIndex(i)
            if s.ShapeType == "com.sun.star.presentation." + kind:
                return s
        return None

    def slide(self, selector):
        sel = selector.strip().lower()
        notes = sel.endswith(".notes")
        if notes:
            sel = sel[: -len(".notes")]
        # office-host's own wording, so a model reads the same refusal
        # whichever helper is behind the handle.
        if not (sel.startswith("s") and sel[1:].isdigit()):
            raise Refused("a slide selector looks like s3, not %s" % selector)
        n = int(sel[1:])
        pages = self.pages()
        if n < 1 or n > pages.getCount():
            raise Refused("slide %d does not exist: the deck has %d" % (n, pages.getCount()))
        return pages.getByIndex(n - 1), notes

    def title_of(self, page):
        t = self.shape_of(page, "TitleTextShape")
        return t.getString() if t is not None else ""

    def read_deck(self):
        pages = self.pages()
        out = "slides=%d" % pages.getCount()
        for i in range(min(pages.getCount(), 40)):
            out += " | s%d: %s" % (i + 1, trunc(self.title_of(pages.getByIndex(i))))
        return out

    def read_slide(self, selector):
        page, notes = self.slide(selector)
        if notes:
            ns = self.shape_of(page.getNotesPage(), "NotesShape")
            return "notes: %s" % (trunc(ns.getString()) if ns is not None and ns.getString() else "(none)")
        texts = []
        for i in range(page.getCount()):
            s = page.getByIndex(i)
            try:
                t = s.getString()
            except Exception:
                t = ""
            if t:
                texts.append(trunc(t))
        return "shapes=%d%s" % (page.getCount(), "".join(" | " + t for t in texts))

    @staticmethod
    def _blank(page):
        for i in range(page.getCount()):
            try:
                if page.getByIndex(i).getString():
                    return False
            except Exception:
                return False
        return True

    def create_slide(self, title, bullets, layout):
        key = (layout or "").strip().lower()
        if key not in LAYOUTS:
            raise Refused("createSlide does not know layout %s: it knows title, titleContent, "
                          "sectionHeader, twoContent, comparison, titleOnly, blank" % layout)
        pages = self.pages()
        # A LibreOffice deck cannot have zero slides, so a "new" deck arrives
        # with one blank one. The first slide made goes into it rather than
        # after it, so an empty deck built up looks the same as in Office.
        first = pages.getCount() == 1 and self._blank(pages.getByIndex(0))
        page = pages.getByIndex(0) if first else pages.insertNewByIndex(pages.getCount() - 1)
        page.Layout = LAYOUTS[key]
        if title:
            t = self.shape_of(page, "TitleTextShape")
            if t is not None:
                t.setString(title)
        items = [b for b in parse_grid(bullets or "")[0] if b != ""]
        if items:
            body = self.shape_of(page, "OutlinerShape") or self.shape_of(page, "SubtitleShape")
            if body is None:
                body = self.doc.createInstance("com.sun.star.drawing.TextShape")
                page.add(body)
                size = uno.createUnoStruct("com.sun.star.awt.Size")
                size.Width, size.Height = 21000, 11000
                body.setSize(size)
            levels, lines = [], []
            for b in items:
                depth = 0
                while b.startswith(">"):
                    depth += 1
                    b = b[1:].lstrip()
                lines.append(b)
                levels.append(min(depth, 4))
            body.setString("\n".join(lines))
            e = body.getText().createEnumeration()
            i = 0
            while e.hasMoreElements() and i < len(levels):
                p = e.nextElement()
                try:
                    p.NumberingLevel = levels[i]
                except Exception:
                    pass
                i += 1
        return "slide %d added, layout %s, %d bullet(s)" % (pages.getCount(), layout, len(items))

    def write(self, selector, text):
        page, notes = self.slide(selector)
        if notes:
            ns = self.shape_of(page.getNotesPage(), "NotesShape")
            if ns is None:
                raise Refused("slide %s has no notes placeholder" % selector)
            ns.setString(text or "")
            return "notes on %s written (%d chars)" % (selector, len(text or ""))
        t = self.shape_of(page, "TitleTextShape")
        if t is None:
            raise Refused("slide %s has no title placeholder" % selector)
        t.setString(text or "")
        return "title on %s written (%d chars)" % (selector, len(text or ""))


# ---------------------------------------------------------------- dispatch

FILTERS = {
    "excel": {"pdf": "calc_pdf_Export", "xlsx": "Calc MS Excel 2007 XML", "csv": "Text - txt - csv (StarCalc)"},
    "word": {"pdf": "writer_pdf_Export", "docx": "MS Word 2007 XML"},
    "powerpoint": {"pdf": "impress_pdf_Export", "pptx": "Impress MS PowerPoint 2007 XML"},
}
DEFAULT_FORMAT = {"excel": "xlsx", "word": "docx", "powerpoint": "pptx"}


def export(app, doc, fmt, path):
    if not path:
        raise Refused("export needs a path")
    fmt = (fmt or "").lower()
    if fmt == "png":
        raise Refused("export png (charts to pictures) needs Excel; this LibreOffice helper does xlsx, pdf and csv")
    filters = FILTERS[app]
    flt = filters.get(fmt) or filters[DEFAULT_FORMAT[app]]
    full = os.path.abspath(path)
    os.makedirs(os.path.dirname(full) or ".", exist_ok=True)
    doc.storeToURL(uno.systemPathToFileUrl(full), (prop("FilterName", flt),))
    return "exported %s" % full


def handle_line(office, line):
    try:
        msg = json.loads(line)
    except ValueError as e:
        return reply_fail("unparseable request: %s" % e)
    method = msg.get("method", "")
    handle = msg.get("handle", "")
    args = msg.get("args") or {}
    payload = msg.get("payload", "")
    selector = args.get("selector", "")
    app = office.app
    try:
        if method == "open":
            return reply_ok(office.open(args.get("path", "")))
        doc = office.find(handle)
        if doc is None:
            noun = {"excel": "workbook", "word": "doc", "powerpoint": "presentation"}[app]
            raise Refused("%s not open for %s" % (noun, handle))
        if method == "export":
            return reply_ok(export(app, doc, args.get("format", ""), args.get("path", "")))
        if app == "excel":
            c = Calc(doc)
            out = {
                "read": lambda: c.read(selector),
                "write": lambda: c.write(selector, payload),
                "format": lambda: c.format(selector, payload),
                "addSheet": lambda: c.add_sheet(args.get("name", "")),
                "chart": lambda: c.chart(args.get("kind", ""), args.get("source", ""), args.get("title", ""),
                                         args.get("at", ""), payload),
            }.get(method)
        elif app == "word":
            w = Writer(doc)

            def word_read():
                if selector in ("body", ""):
                    return "paras=%d" % len(w.paras())
                if selector.startswith("p") and selector[1:].isdigit():
                    n = int(selector[1:])
                    return "para %d: %s" % (n, trunc(w.para(n).getString()))
                raise Refused("word selector %s: use body, or p0, p1 ... for one paragraph" % selector)

            def word_write():
                if not (selector.startswith("p") and selector[1:].isdigit()):
                    raise Refused("word write needs a paragraph selector like p3")
                n = int(selector[1:])
                w.para(n).setString(payload)
                return "para %d written (%d chars)" % (n, len(payload))

            out = {
                "read": word_read,
                "write": word_write,
                "insertParagraph": lambda: w.insert_paragraph(payload, args.get("name", "")),
                "insertTable": lambda: w.insert_table(payload, args.get("name", "")),
                "pageBreak": lambda: w.page_break(args.get("name", "")),
            }.get(method)
        else:
            p = Impress(doc)
            out = {
                "read": lambda: p.read_deck() if selector in ("deck", "") else p.read_slide(selector),
                "write": lambda: p.write(selector, payload),
                "createSlide": lambda: p.create_slide(args.get("title", ""), payload, args.get("name", "")),
            }.get(method)
        if out is None:
            raise Refused("unsupported %s.%s on the LibreOffice helper" % (app, method))
        return reply_ok(out())
    except Refused as e:
        return reply_fail(str(e))
    except Exception as e:  # a UNO failure: say what it was, keep serving
        msg = getattr(e, "Message", "") or str(e) or type(e).__name__
        trace("error in %s: %r" % (method, e))
        return reply_fail("%s failed: %s" % (method, msg))


def serve(office, path):
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(path)
    os.chmod(path, 0o600)  # this user's helper, nobody else's
    srv.listen(1)
    print("lo-host live: app=%s pipe=%s" % (office.app, path), flush=True)
    while True:
        conn, _ = srv.accept()
        trace("client connected")
        with conn, conn.makefile("r", encoding="utf-8", newline="\n") as rd, \
                conn.makefile("w", encoding="utf-8", newline="\n") as wr:
            for line in rd:
                if not line.strip():
                    continue
                trace("<- " + line.strip()[:200])
                out = handle_line(office, line)
                trace("-> " + out[:200])
                wr.write(out + "\n")
                wr.flush()
        trace("client gone; waiting for the next")


def main():
    global TRACE
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--pipe", required=True)
    ap.add_argument("--app", required=True, choices=["excel", "word", "powerpoint"])
    ap.add_argument("--trace", action="store_true")
    a = ap.parse_args()
    TRACE = a.trace or os.environ.get("AGENT_TRACE") == "1"
    visible = os.environ.get("AGENT_LO_VISIBLE") == "1" and bool(os.environ.get("DISPLAY"))
    office = Office(a.app, visible)
    path = socket_path(a.pipe)

    def shutdown(*_):
        try:
            os.unlink(path)
        except OSError:
            pass
        office.stop()
        os._exit(0)

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)
    # A parent killed outright (SIGKILL) sends nothing. Watch for being
    # orphaned instead, so LibreOffice never outlives the server that
    # wanted it.
    parent = os.getppid()

    def watch():
        while True:
            time.sleep(1)
            if os.getppid() != parent:
                shutdown()

    threading.Thread(target=watch, daemon=True).start()
    try:
        serve(office, path)
    finally:
        shutdown()


if __name__ == "__main__":
    main()
