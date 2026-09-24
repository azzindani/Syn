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
import re
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


# How many places `find` lists before it says "and more", as office-host.
FIND_SHOWN = 20


def refuse(msg):
    raise Refused(msg)


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
    """One LibreOffice instance, started and owned by this helper -- or, when
    a helper for the same pipe died without stopping it, adopted from it."""

    def __init__(self, app, visible, pipe=None):
        self.app = app
        self.visible = visible
        self.proc = None
        self.profile = None
        local = uno.getComponentContext()
        resolver = local.ServiceManager.createInstanceWithContext("com.sun.star.bridge.UnoUrlResolver", local)
        if pipe:
            # Named after the helper's own pipe, not this process, so the
            # next helper on that pipe can find it. A helper killed outright
            # (SIGKILL skips every handler) used to leave its LibreOffice
            # running with the document open and locked; the replacement
            # started a second one, which could not open the file. Office's
            # own rule is the one to copy: attach to the running app.
            self.pipe = "synlo_" + re.sub(r"[^A-Za-z0-9_]", "_", pipe)
            ctx = self._resolve(resolver, time.time() + 1)
            if ctx is not None:
                self.ctx = ctx
                self.desktop = ctx.ServiceManager.createInstanceWithContext("com.sun.star.frame.Desktop", ctx)
                trace("adopted the LibreOffice already on %s" % self.pipe)
                return
        else:
            self.pipe = "synlo_%s_%d" % (app, os.getpid())
        self.profile = tempfile.mkdtemp(prefix="syn-lo-%s-" % app)
        args = [shutil.which("soffice") or "/usr/lib/libreoffice/program/soffice",
                "--norestore", "--nologo", "--nodefault", "--nolockcheck",
                "-env:UserInstallation=" + uno.systemPathToFileUrl(self.profile),
                "--accept=pipe,name=%s;urp;" % self.pipe]
        if not visible:
            args[1:1] = ["--headless", "--invisible"]
        # Its own session, so stopping it takes the whole tree: `soffice`
        # is a script that starts oosplash that starts soffice.bin.
        self.proc = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL, start_new_session=True)
        self.ctx = self._resolve(resolver, time.time() + 90)
        if self.ctx is None:
            raise RuntimeError("LibreOffice did not answer on its pipe within 90s")
        self.desktop = self.ctx.ServiceManager.createInstanceWithContext("com.sun.star.frame.Desktop", self.ctx)
        trace("LibreOffice ready on %s" % self.pipe)

    def _resolve(self, resolver, deadline):
        while True:
            try:
                return resolver.resolve("uno:pipe,name=%s;urp;StarOffice.ComponentContext" % self.pipe)
            except Exception:
                if self.proc is not None and self.proc.poll() is not None:
                    raise RuntimeError("LibreOffice exited (%s) before it was ready" % self.proc.returncode)
                if time.time() > deadline:
                    return None
                time.sleep(0.3)

    def stop(self):
        if self.proc is None:
            # Adopted: no process of ours to signal, so ask it to quit.
            try:
                self.desktop.terminate()
            except Exception:
                pass
            return
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


    # ---- the table-driven verbs ------------------------------------------

    def target(self, selector, verb):
        """A range and its shape: whole rows (5:7), whole columns (C:E), or
        cells, as office-host tells them apart."""
        sheet, addr = split_range(selector)
        if not addr:
            raise Refused("%s needs a range like Sheet!A1:D20, rows like Sheet!5:7 or columns like Sheet!C:E" % verb)
        ws = self.sheet(sheet)
        a = addr.replace("$", "")
        if re.fullmatch(r"\d+(:\d+)?", a):
            kind, a = "rows", a if ":" in a else "%s:%s" % (a, a)
        elif re.fullmatch(r"[A-Za-z]{1,3}(:[A-Za-z]{1,3})?", a):
            kind, a = "columns", a if ":" in a else "%s:%s" % (a, a)
        else:
            kind = "cells"
        return ws, self.rng(ws, a), kind, sheet, a

    def insert(self, selector):
        ws, r, kind, sheet, a = self.target(selector, "insert")
        ra = r.getRangeAddress()
        if kind == "rows":
            ws.Rows.insertByIndex(ra.StartRow, ra.EndRow - ra.StartRow + 1)
        elif kind == "columns":
            ws.Columns.insertByIndex(ra.StartColumn, ra.EndColumn - ra.StartColumn + 1)
        else:
            ws.insertCells(ra, uno.Enum("com.sun.star.sheet.CellInsertMode", "DOWN"))
        return "inserted %s at %s!%s; what was there moved %s" % (kind, sheet, a, "right" if kind == "columns" else "down")

    def delete(self, selector):
        ws, r, kind, sheet, a = self.target(selector, "delete")
        ra = r.getRangeAddress()
        if kind == "rows":
            ws.Rows.removeByIndex(ra.StartRow, ra.EndRow - ra.StartRow + 1)
        elif kind == "columns":
            ws.Columns.removeByIndex(ra.StartColumn, ra.EndColumn - ra.StartColumn + 1)
        else:
            ws.removeRange(ra, uno.Enum("com.sun.star.sheet.CellDeleteMode", "UP"))
        return "deleted %s %s!%s; what was after them moved %s" % (kind, sheet, a, "left" if kind == "columns" else "up")

    def header_column(self, r, header, verb):
        a = r.getRangeAddress()
        names = [r.getCellByPosition(j, 0).getString() for j in range(a.EndColumn - a.StartColumn + 1)]
        for j, n in enumerate(names):
            if n.strip().lower() == header.strip().lower():
                return j
        raise Refused("%s: no column headed %s in the first row of the range; its headers are %s"
                      % (verb, header, ", ".join(names)))

    def sort(self, selector, header, rule):
        ws, r, kind, sheet, a = self.target(selector, "sort")
        col = self.header_column(r, header, "sort")
        rule = (rule or "").strip().lower()
        if rule not in ("", "asc", "ascending", "desc", "descending"):
            raise Refused("sort: rule is asc or desc, not %s" % rule)
        asc = not rule.startswith("desc")
        field = uno.createUnoStruct("com.sun.star.table.TableSortField")
        field.Field = col
        field.IsAscending = asc
        desc = list(r.createSortDescriptor())
        for p in desc:
            if p.Name == "SortFields":
                p.Value = uno.Any("[]com.sun.star.table.TableSortField", (field,))
            elif p.Name == "ContainsHeader":
                p.Value = True
        uno.invoke(r, "sort", (tuple(desc),))
        return "sorted %s!%s by %s, %s first" % (sheet, a, header, "smallest" if asc else "largest")

    def copy(self, source, at):
        ss, sa = split_range(source)
        ds, da = split_range(at)
        if not sa:
            raise Refused("copy needs a source like data!A1:D20")
        if not da:
            raise Refused("copy needs a destination cell like Summary!A1")
        src = self.rng(self.sheet(ss), sa)
        dws = self.sheet(ds)
        dst = self.rng(dws, da).getCellByPosition(0, 0)
        dws.copyRange(dst.getCellAddress(), src.getRangeAddress())
        a = src.getRangeAddress()
        return "copied %s!%s to %s!%s (%dx%d: values, formulas and formats)" % (
            ss, sa, ds, da, a.EndRow - a.StartRow + 1, a.EndColumn - a.StartColumn + 1)

    def sheet_op(self, selector, action, name):
        sheet, _ = split_range(selector)
        ws = self.sheet(sheet)
        sheets = self.doc.Sheets
        if action == "rename":
            ws.Name = name
            return "sheet %s is now %s" % (sheet, name)
        if action == "delete":
            if sheets.getCount() == 1:
                raise Refused("a workbook keeps at least one sheet: add another before deleting this one")
            sheets.removeByName(sheet)
            return "sheet %s deleted" % sheet
        if action == "copy":
            idx = list(sheets.ElementNames).index(sheet)
            sheets.copyByName(sheet, name, idx + 1)
            return "sheet %s copied as %s" % (sheet, name)
        if action in ("hide", "show"):
            ws.IsVisible = action == "show"
            return "sheet %s %s" % (sheet, "shown" if action == "show" else "hidden")
        raise Refused("sheet does not know %s: rename, delete, copy, hide or show" % action)

    def comment(self, selector, text):
        sheet, addr = split_range(selector)
        if not addr:
            raise Refused("comment needs a cell like data!B2")
        ws = self.sheet(sheet)
        cell = self.rng(ws, addr).getCellByPosition(0, 0)
        ws.Annotations.insertNew(cell.getCellAddress(), text)
        return "note on %s!%s" % (sheet, addr.split(":")[0])

    def ranges(self, selector):
        sheet, addr = split_range(selector or "")
        names = [sheet] if sheet else list(self.doc.Sheets.ElementNames)
        for n in names:
            ws = self.sheet(n)
            if addr:
                yield n, self.rng(ws, addr)
            else:
                cur = ws.createCursor()
                cur.gotoStartOfUsedArea(False)
                cur.gotoEndOfUsedArea(True)
                yield n, cur

    def find(self, selector, text):
        hits, more = [], False
        want = text.lower()
        for name, r in self.ranges(selector):
            a = r.getRangeAddress()
            for i in range(a.EndRow - a.StartRow + 1):
                for j in range(a.EndColumn - a.StartColumn + 1):
                    if want in r.getCellByPosition(j, i).getString().lower():
                        if len(hits) == FIND_SHOWN:
                            more = True
                            break
                        ref = ("'%s'" % name) if " " in name else name
                        hits.append("%s!%s%d" % (ref, col_name(a.StartColumn + j), a.StartRow + i + 1))
                if more:
                    break
            if more:
                break
        if not hits:
            return "no cell shows %s" % text
        return "%s is in %d%s cell(s): %s" % (text, len(hits), "+" if more else "", ", ".join(hits))

    def replace(self, selector, text, with_):
        count = 0
        for _, r in self.ranges(selector):
            rd = r.createReplaceDescriptor()
            rd.SearchString = text
            rd.ReplaceString = with_
            rd.SearchCaseSensitive = False
            count += r.replaceAll(rd)
        if not count:
            return "no cell contains %s; nothing changed" % text
        return "replaced %s with %s in %d cell(s)" % (text, with_, count)


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


    def para_span(self, selector, verb):
        m = re.fullmatch(r"p(\d+)(?::p?(\d+))?", (selector or "").strip(), re.I)
        if not m:
            raise Refused("%s needs a paragraph like p3, or p3:p5 for several" % verb)
        a = int(m.group(1))
        b = int(m.group(2)) if m.group(2) else a
        n = len(self.paras())
        if b < a or b >= n:
            raise Refused("%s: the document has paragraphs p0 to p%d" % (verb, n - 1))
        return a, b

    def insert_paragraph_at(self, text, style, at):
        n, _ = self.para_span(at, "insertParagraph")
        t = self.doc.Text
        from com.sun.star.text.ControlCharacter import PARAGRAPH_BREAK
        cur = t.createTextCursorByRange(self.paras()[n].getStart())
        t.insertString(cur, text or "", False)
        t.insertControlCharacter(cur, PARAGRAPH_BREAK, False)
        note = self.style_note(self.paras()[n], style or "Normal")
        return "paragraph inserted as p%d, %d chars%s; the ones from p%d on moved down one" % (n, len(text or ""), note, n)

    def delete(self, selector):
        a, b = self.para_span(selector, "delete")
        ps = self.paras()
        t = self.doc.Text
        cur = t.createTextCursorByRange(ps[a].getStart())
        cur.gotoRange(ps[b].getEnd(), True)
        if b + 1 < len(ps):
            cur.goRight(1, True)  # and the break after, so no empty paragraph is left
        elif a > 0:
            cur.gotoRange(ps[a - 1].getEnd(), False)
            cur.gotoRange(ps[b].getEnd(), True)
        cur.setString("")
        return "deleted %s; the paragraphs after moved up, so p%d is now what followed" % (
            "p%d" % a if a == b else "p%d to p%d" % (a, b), a)

    def find(self, text):
        want = text.lower()
        hits = ["p%d" % i for i, p in enumerate(self.paras()) if want in p.getString().lower()]
        more = len(hits) > FIND_SHOWN
        hits = hits[:FIND_SHOWN]
        if not hits:
            return "%s does not appear in the document" % text
        return "%s appears in %d%s paragraph(s): %s" % (text, len(hits), "+" if more else "", ", ".join(hits))

    def replace(self, text, with_):
        rd = self.doc.createReplaceDescriptor()
        rd.SearchString = text
        rd.ReplaceString = with_
        rd.SearchCaseSensitive = False
        n = self.doc.replaceAll(rd)
        if not n:
            return "%s does not appear in the document; nothing changed" % text
        return "replaced %s with %s, %d time(s)" % (text, with_, n)

    def comment(self, selector, text):
        a, _ = self.para_span(selector, "comment")
        note = self.doc.createInstance("com.sun.star.text.textfield.Annotation")
        note.Content = text
        note.Author = "Syn"
        t = self.doc.Text
        t.insertTextContent(t.createTextCursorByRange(self.paras()[a].getStart()), note, False)
        return "comment on p%d" % a

    def header(self, which, text):
        which = (which or "header").strip().lower()
        if which not in ("header", "footer"):
            raise Refused("header takes name header or footer, not %s" % which)
        name = self.paras()[0].PageStyleName or "Standard"
        style = self.doc.StyleFamilies.getByName("PageStyles").getByName(name)
        if which == "header":
            style.HeaderIsOn = True
            style.HeaderText.setString(text)
        else:
            style.FooterIsOn = True
            style.FooterText.setString(text)
        return "%s set" % which


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

    def number(self, selector, verb):
        m = re.fullmatch(r"s(\d+)", (selector or "").strip(), re.I)
        if not m:
            raise Refused("%s needs a slide like s3" % verb)
        n = int(m.group(1))
        count = self.pages().getCount()
        if n < 1 or n > count:
            raise Refused("slide %d does not exist: the deck has %d" % (n, count))
        return n

    def delete(self, selector):
        n = self.number(selector, "delete")
        self.pages().remove(self.pages().getByIndex(n - 1))
        return "slide %d deleted; the slides after it moved up one, and the deck has %d" % (n, self.pages().getCount())

    def duplicate(self, selector):
        n = self.number(selector, "duplicateSlide")
        self.doc.duplicate(self.pages().getByIndex(n - 1))
        return "slide %d duplicated as s%d; the slides after it moved down one" % (n, n + 1)

    def text_box(self, selector, box, text, style):
        n = self.number(selector, "textBox")
        page = self.pages().getByIndex(n - 1)
        vals = [60.0, 140.0, 600.0, 60.0]
        for i, part in enumerate((box or "").split(",")[:4]):
            try:
                vals[i] = float(part.strip())
            except ValueError:
                pass
        hmm = lambda pt: int(pt * 2540 / 72)  # points to 1/100 mm
        shape = self.doc.createInstance("com.sun.star.drawing.TextShape")
        page.add(shape)
        shape.Position = uno.createUnoStruct("com.sun.star.awt.Point", hmm(vals[0]), hmm(vals[1]))
        shape.Size = uno.createUnoStruct("com.sun.star.awt.Size", hmm(vals[2]), hmm(vals[3]))
        shape.setString(text)
        for pair in (style or "").split(";"):
            if "=" not in pair:
                continue
            k, v = [x.strip() for x in pair.split("=", 1)]
            if k.lower() == "size":
                shape.CharHeight = float(v)
            elif k.lower() == "bold":
                shape.CharWeight = 150.0 if truthy(v) else 100.0
            else:
                raise Refused("textBox style %s is not implemented on the LibreOffice helper" % k)
        return "text box on s%d at %d,%d (%dx%d), %d chars" % (n, vals[0], vals[1], vals[2], vals[3], len(text))

    def find(self, text):
        want, hits = text.lower(), []
        for i in range(self.pages().getCount()):
            page = self.pages().getByIndex(i)
            for j in range(page.getCount()):
                sh = page.getByIndex(j)
                if hasattr(sh, "getString") and want in sh.getString().lower():
                    hits.append("s%d" % (i + 1))
                    break
        return "%s is on %s" % (text, ", ".join(hits)) if hits else "%s is on no slide" % text

    def replace(self, text, with_):
        count = 0
        for i in range(self.pages().getCount()):
            page = self.pages().getByIndex(i)
            rd = page.createReplaceDescriptor()
            rd.SearchString = text
            rd.ReplaceString = with_
            rd.SearchCaseSensitive = False
            count += page.replaceAll(rd)
        if not count:
            return "%s is on no slide; nothing changed" % text
        return "replaced %s with %s, %d time(s)" % (text, with_, count)


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
                "insert": lambda: c.insert(selector),
                "delete": lambda: c.delete(selector),
                "sort": lambda: c.sort(selector, args.get("name", ""), args.get("rule", "")),
                "copy": lambda: c.copy(args.get("source", ""), args.get("at", "")),
                "sheet": lambda: c.sheet_op(selector, args.get("action", ""), args.get("name", "")),
                "comment": lambda: c.comment(selector, payload),
                "find": lambda: c.find(selector, args.get("text", "")),
                "replace": lambda: c.replace(selector, args.get("text", ""), args.get("with", "")),
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
                "insertParagraph": lambda: (w.insert_paragraph_at(payload, args.get("name", ""), args["at"])
                                            if args.get("at") else w.insert_paragraph(payload, args.get("name", ""))),
                "insertTable": lambda: (refuse("insertTable at a position is not implemented on the LibreOffice helper")
                                        if args.get("at") else w.insert_table(payload, args.get("name", ""))),
                "delete": lambda: w.delete(selector),
                "find": lambda: w.find(args.get("text", "")),
                "replace": lambda: w.replace(args.get("text", ""), args.get("with", "")),
                "comment": lambda: w.comment(selector, payload),
                "header": lambda: w.header(args.get("name", ""), payload),
                "pageBreak": lambda: w.page_break(args.get("name", "")),
            }.get(method)
        else:
            p = Impress(doc)
            out = {
                "read": lambda: p.read_deck() if selector in ("deck", "") else p.read_slide(selector),
                "write": lambda: p.write(selector, payload),
                "createSlide": lambda: p.create_slide(args.get("title", ""), payload, args.get("name", "")),
                "delete": lambda: p.delete(selector),
                "duplicateSlide": lambda: p.duplicate(selector),
                "textBox": lambda: p.text_box(selector, args.get("name", ""), payload, args.get("style", "")),
                "find": lambda: p.find(args.get("text", "")),
                "replace": lambda: p.replace(args.get("text", ""), args.get("with", "")),
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


# How many clients may be connected at once: the console, a couple of MCP
# clients and a terminal. One used to be the limit, and a second client --
# Claude Desktop holding Excel while the console asked for it -- waited in
# the listen backlog with no answer until the first went away.
MAX_CLIENTS = 8

# LibreOffice is driven by one caller at a time. Clients interleave at the
# granularity of one request line, exactly as they would through Excel.
_ONE_AT_A_TIME = threading.Lock()


def already_served(path):
    """Whether a live helper is already answering on `path`. Binding over it
    would unlink its socket and leave it serving nobody."""
    c = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    c.settimeout(2)
    try:
        c.connect(path)
        return True
    except OSError:
        return False
    finally:
        c.close()


def client(office, conn):
    trace("client connected")
    try:
        with conn, conn.makefile("r", encoding="utf-8", newline="\n") as rd, \
                conn.makefile("w", encoding="utf-8", newline="\n") as wr:
            for line in rd:
                if not line.strip():
                    continue
                trace("<- " + line.strip()[:200])
                with _ONE_AT_A_TIME:
                    out = handle_line(office, line)
                trace("-> " + out[:200])
                wr.write(out + "\n")
                wr.flush()
    except OSError as e:
        # A client that vanishes mid-reply ends its connection, not the
        # helper.
        trace("client dropped: %s" % e)
    trace("client gone")


def serve(office, path):
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(path)
    os.chmod(path, 0o600)  # this user's helper, nobody else's
    srv.listen(MAX_CLIENTS)
    print("lo-host live: app=%s pipe=%s" % (office.app, path), flush=True)
    live = []
    while True:
        conn, _ = srv.accept()
        live = [t for t in live if t.is_alive()]
        if len(live) >= MAX_CLIENTS:
            # Say so rather than leave it hanging on a read that never ends.
            try:
                conn.sendall((json.dumps({"ok": False, "error": "busy: %d clients are already connected to this helper" % MAX_CLIENTS}) + "\n").encode())
            finally:
                conn.close()
            continue
        t = threading.Thread(target=client, args=(office, conn), daemon=True)
        t.start()
        live.append(t)


def main():
    global TRACE
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--pipe", required=True)
    ap.add_argument("--app", required=True, choices=["excel", "word", "powerpoint"])
    ap.add_argument("--trace", action="store_true")
    a = ap.parse_args()
    TRACE = a.trace or os.environ.get("AGENT_TRACE") == "1"
    visible = os.environ.get("AGENT_LO_VISIBLE") == "1" and bool(os.environ.get("DISPLAY"))
    path = socket_path(a.pipe)
    # Before LibreOffice is started, and before the shutdown below is
    # armed: that unlinks the socket, which here is the other helper's.
    if already_served(path):
        print("lo-host: another helper is already serving %s; not taking it over" % path, flush=True)
        sys.exit(3)
    office = Office(a.app, visible, a.pipe)

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
