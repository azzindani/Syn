// Undo for a live document.
//
// `undo` was on the MCP surface and in the loop's tools from the start, and
// every live Office document answered it with "unsupported excel.undo": the
// relay's snapshots are of the in-memory model, and the `.bak` this helper
// wrote before each change held the handle's name, not the data. A model
// that made a mistake had no way back but the human's Ctrl+Z.
//
// Each application gets the undo it can actually do:
//
// - Word records automation edits in its own undo stack. Each call is
//   wrapped in one custom undo record (UndoRecord, Word 2010+), so one Syn
//   call is one Document.Undo step, however many edits it made.
// - Excel does not: a change made through COM empties Excel's undo stack
//   instead of joining it. So this helper keeps its own: the cells a call
//   is about to change are copied to a hidden scratch workbook this helper
//   creates (and is therefore allowed to close), and sheets, charts,
//   pivots, tables, names and pictures the call adds are deleted again.
// - PowerPoint has no Undo in its object model. A copy of the deck is saved
//   before each call (SaveCopyAs, which leaves the open deck where it is),
//   and undo puts back the slides the call added, removed, moved or changed
//   from that copy.
//
// All three refuse rather than guess when the document changed after Syn's
// last call to it -- the person typed in it -- because undoing then would
// take their work away with Syn's. They say to use Ctrl+Z instead.
//
// Only this helper's calls are undone, newest first, per document, up to
// UndoDepth of them. Nothing here saves or closes the user's document.
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        private const int UndoDepth = 20;

        /// <summary>Cells a single Excel call may change and still be undone.
        /// The snapshot is a copy of them; a fill down 250,000 rows copies
        /// 250,000 rows.</summary>
        private const long UndoCellCap = 250_000;

        private sealed class UndoEntry
        {
            public string What = "";
            /// <summary>Puts the document back. Null when the call cannot be
            /// undone; `Why` says so.</summary>
            public Func<dynamic, string>? Revert;
            public string? Why;
            /// <summary>The document as the call left it. Undo compares this
            /// with the document now; a difference means someone else
            /// changed it since.</summary>
            public string? After;
            /// <summary>Files to delete when the entry is gone.</summary>
            public List<string> Files = new();
            /// <summary>Word: a record made on the way to something else (the
            /// field refresh before an export). Undone along with the entry
            /// under it, never reported as the thing undone.</summary>
            public bool Silent;
            /// <summary>PowerPoint: slide ids this entry names, rewritten when
            /// an undo puts a slide back under a new id.</summary>
            public List<int> SlideIds = new();
            /// <summary>Excel: sheets of the scratch workbook this entry keeps.</summary>
            public List<string> ScratchSheets = new();
            /// <summary>How `After` is taken, so undo can take it again.</summary>
            public Func<object, string>? Print;
        }

        /// <summary>The application this helper drives, for dropping an
        /// entry's scratch sheets when it falls off the stack.</summary>
        private static object? _appRef;

        private static readonly Dictionary<string, List<UndoEntry>> Undos = new();

        private static List<UndoEntry> StackFor(string doc)
        {
            if (!Undos.TryGetValue(doc, out var s)) Undos[doc] = s = new List<UndoEntry>();
            return s;
        }

        private static void Push(string doc, UndoEntry e)
        {
            var s = StackFor(doc);
            s.Add(e);
            while (s.Count > UndoDepth)
            {
                Drop(s[0]);
                s.RemoveAt(0);
            }
        }

        private static void Drop(UndoEntry e)
        {
            foreach (var f in e.Files)
                try { File.Delete(f); } catch { /* temp, best-effort */ }
            if (_appRef != null)
                foreach (var sh in e.ScratchSheets) DropScratchSheet(_appRef, sh);
        }

        /// <summary>Methods that change nothing, and so leave nothing to undo.</summary>
        private static bool ReadOnly(string method, string line) =>
            method is "read" or "find" or "open" or "undo"
            || (method == "export" && _app != "word")
            || (method == "macro" && JsonField(JsonField(line, "args"), "action").ToLowerInvariant() is "read" or "list");

        private static bool IsOk(string reply) => reply.StartsWith("{\"ok\":true", StringComparison.Ordinal);

        private static string Hash(string s)
        {
            var b = SHA256.HashData(Encoding.UTF8.GetBytes(s));
            return Convert.ToHexString(b, 0, 12);
        }

        /// <summary>Run one call with its undo recorded around it. Everything
        /// that changes a document passes through here.</summary>
        private static string WithUndo(object app, string method, string handle, string line, string selector,
            Func<string> call)
        {
            _appRef = app;
            if (ReadOnly(method, line)) return call();
            Func<string, string>? after = null;
            string? doc = null;
            try
            {
                (doc, after) = _app switch
                {
                    "word" => WordBefore(app, handle, method),
                    "excel" => ExcelBefore(app, handle, method, line, selector),
                    "powerpoint" => PptBefore(app, handle, method, selector),
                    _ => ((string?)null, (Func<string, string>?)null),
                };
            }
            catch (Exception e)
            {
                // The call still runs: a document that cannot be snapshotted
                // is still one a person asked to change. It just cannot be
                // undone, and undo will say why.
                var failed = new UndoEntry { What = method, Why = $"its undo could not be prepared ({e.Message})" };
                try { doc = DocKey(app, handle); } catch { doc = null; }
                var r = call();
                if (doc != null && IsOk(r)) Push(doc, failed);
                return r;
            }
            var reply = call();
            if (after != null)
            {
                try { after(reply); }
                catch (Exception e) { Trace($"undo: recording after {method} failed: {e.Message}"); }
            }
            return reply;
        }

        /// <summary>The document a handle names, as the key its undo stack
        /// lives under: the app and the document's name, whatever unit the
        /// handle carries.</summary>
        private static string DocKey(object appO, string handle)
        {
            dynamic app = appO;
            string name = _app switch
            {
                "word" => (string)(FindWordDoc(app, handle) ?? throw new InvalidOperationException("doc not open")).Name,
                "excel" => (string)(FindWorkbook(app, handle) ?? throw new InvalidOperationException("workbook not open")).Name,
                _ => (string)(FindPresentation(app, handle) ?? throw new InvalidOperationException("presentation not open")).Name,
            };
            return _app + ":" + name;
        }

        /// <summary>`undo`: put back what this helper's last call to this
        /// document changed.</summary>
        private static string UndoLast(object appO, string handle) =>
            Guarded(() =>
            {
                dynamic app = appO;
                _appRef = appO;
                var key = DocKey(appO, handle);
                var s = StackFor(key);
                if (s.Count == 0)
                    return Fail($"nothing to undo in {key[(key.IndexOf(':') + 1)..]}: Syn has not changed it since this helper started");
                dynamic doc = _app switch
                {
                    "word" => FindWordDoc(app, handle)!,
                    "excel" => FindWorkbook(app, handle)!,
                    _ => FindPresentation(app, handle)!,
                };
                var top = s[^1];
                if (top.After != null && top.Print != null)
                {
                    var now = top.Print((object)doc);
                    if (now != top.After)
                        return Fail("the document changed after Syn's last edit (someone typed or edited in it), " +
                                    "so undoing now could take their work away too. Undo in the application " +
                                    "itself (Ctrl+Z), or ask the person first");
                }
                var done = new List<string>();
                // Word's silent records (a field refresh) come off with the
                // entry beneath them.
                while (s.Count > 0)
                {
                    var e = s[^1];
                    if (e.Revert == null)
                    {
                        if (done.Count > 0) break;
                        return Fail($"the last change ({e.What}) cannot be undone here: {e.Why}");
                    }
                    var below = s.Count > 1 ? s[^2] : null;
                    var said = e.Revert(doc);
                    // A field refresh that left nothing in Word's undo list
                    // makes that Undo take back Syn's previous edit instead.
                    // The document then no longer matches what that edit
                    // left, so the step is redone and the entry dropped.
                    if (e.Silent && below?.After != null && below.Print != null && below.Print((object)doc) != below.After)
                        try { doc.Redo(1); } catch { }
                    s.RemoveAt(s.Count - 1);
                    Drop(e);
                    if (!e.Silent)
                    {
                        done.Add(said);
                        break;
                    }
                }
                var left = s.Count(e => !e.Silent);
                return Ok($"undid {string.Join("; ", done)}; {left} more change(s) can be undone");
            });

        // ---------------------------------------------------------------- Word

        private static (string?, Func<string, string>?) WordBefore(object appO, string handle, string method)
        {
            dynamic app = appO;
            dynamic? doc = FindWordDoc(app, handle);
            if (doc == null) return (null, null);
            // An export changes the document only by refreshing its fields.
            // One with none leaves Word's undo list alone, and recording an
            // entry for it would make the next undo take back an older edit.
            if (method == "export" && !HasFields((object)doc)) return (null, null);
            var key = _app + ":" + (string)doc.Name;
            var grouped = true;
            try { app.UndoRecord.StartCustomRecord("Syn " + method); }
            catch { grouped = false; }
            return (key, reply =>
            {
                try { if (grouped) app.UndoRecord.EndCustomRecord(); } catch { }
                if (!IsOk(reply)) return reply;
                var e = new UndoEntry
                {
                    What = method,
                    // The field refresh an export does first is a change of
                    // its own, and it would otherwise be what the next undo
                    // took back instead of the model's own last edit.
                    Silent = method == "export",
                    After = WordPrint((object)doc),
                    Print = WordPrint,
                    Revert = d =>
                    {
                        bool undone = d.Undo(1);
                        if (!undone) throw new InvalidOperationException("Word had nothing to undo: its undo list was cleared (a save, or a macro)");
                        return grouped ? $"the {method}" : $"the last step of the {method} (this Word groups no undo records; press Ctrl+Z for the rest)";
                    },
                };
                Push(key, e);
                return reply;
            });
        }

        private static bool HasFields(object docO)
        {
            dynamic doc = docO;
            try
            {
                if ((int)doc.TablesOfContents.Count > 0 || (int)doc.Fields.Count > 0) return true;
                for (var s = 1; s <= (int)doc.Sections.Count; s++)
                    for (var h = 1; h <= 3; h++)
                    {
                        if ((int)doc.Sections[s].Footers[h].Range.Fields.Count > 0) return true;
                        if ((int)doc.Sections[s].Headers[h].Range.Fields.Count > 0) return true;
                    }
            }
            catch { return true; }
            return false;
        }

        private static string WordPrint(object docO)
        {
            dynamic doc = docO;
            dynamic c = doc.Content;
            string text = c.Text;
            return Hash(text) + "/" + (int)c.End;
        }

        // --------------------------------------------------------------- Excel

        private static dynamic? _scratch;
        private static string _scratchName = "";

        /// <summary>The hidden workbook snapshots live in. This helper made it,
        /// so this helper may close it; it is never saved.</summary>
        private static dynamic Scratch(object appO)
        {
            dynamic app = appO;
            if (_scratch != null)
            {
                try { _ = (string)_scratch.Name; return _scratch; }
                catch { _scratch = null; }
            }
            dynamic wb = app.Workbooks.Add();
            try { wb.Windows[1].Visible = false; } catch { }
            _scratchName = wb.Name;
            wb.Saved = true;
            _scratch = wb;
            return wb;
        }

        private static bool IsScratch(string name) => _scratchName.Length > 0 && name == _scratchName;

        /// <summary>Close the scratch workbook on the way out. It is ours.</summary>
        private static void CloseScratch()
        {
            try { if (_scratch != null) { _scratch.Saved = true; _scratch.Close(false); } } catch { }
            _scratch = null;
        }

        private sealed class RangeSnap
        {
            public string Sheet = "", Addr = "";
            public object? Formulas;
            public string ScratchSheet = "";
            public Dictionary<int, double> Widths = new();
            public Dictionary<int, double> Heights = new();
            public Dictionary<int, bool> RowsHidden = new();
            public Dictionary<int, bool> ColsHidden = new();
        }

        /// <summary>What a workbook holds that a call can add or take away.</summary>
        private sealed class Inventory
        {
            public List<(string name, int visible)> Sheets = new();
            public HashSet<string> Shapes = new(), Tables = new(), Pivots = new(), Names = new(), Slicers = new();
        }

        private static Inventory Take(object wbO)
        {
            dynamic wb = wbO;
            var inv = new Inventory();
            int n = wb.Worksheets.Count;
            for (var i = 1; i <= n; i++)
            {
                dynamic ws = wb.Worksheets[i];
                string sn = ws.Name;
                inv.Sheets.Add((sn, (int)ws.Visible));
                try { for (var k = 1; k <= (int)ws.Shapes.Count; k++) inv.Shapes.Add(sn + "\u0001" + (string)ws.Shapes.Item(k).Name); } catch { }
                try { for (var k = 1; k <= (int)ws.ListObjects.Count; k++) inv.Tables.Add(sn + "\u0001" + (string)ws.ListObjects.Item(k).Name); } catch { }
                try { dynamic pts = ws.PivotTables(); for (var k = 1; k <= (int)pts.Count; k++) inv.Pivots.Add(sn + "\u0001" + (string)pts.Item(k).Name); } catch { }
            }
            try { for (var k = 1; k <= (int)wb.Names.Count; k++) inv.Names.Add((string)wb.Names.Item(k).Name); } catch { }
            try { for (var k = 1; k <= (int)wb.SlicerCaches.Count; k++) inv.Slicers.Add((string)wb.SlicerCaches.Item(k).Name); } catch { }
            return inv;
        }

        /// <summary>The cells a call is about to change, when it names them.</summary>
        private static (string sheet, string addr)? CellsOf(object wbO, string method, string line, string selector)
        {
            dynamic wb = wbO;
            var args = JsonField(line, "args");
            switch (method)
            {
                case "write":
                {
                    var (sheet, addr) = SplitRange(selector);
                    if (addr == "") return null;
                    dynamic target = Sheet(wb, sheet).Range[addr];
                    var grid = ParseGrid(JsonField(line, "payload"));
                    int rows = Math.Max((int)target.Rows.Count, grid.Length);
                    int cols = Math.Max((int)target.Columns.Count, grid.Length == 0 ? 1 : grid.Max(r => r.Length));
                    return (sheet, (string)target.Resize(rows, cols).Address(false, false));
                }
                case "format": case "conditional": case "validate": case "comment": case "link":
                case "sort": case "dedupe": case "filter": case "table":
                {
                    var sel = method == "table" ? JsonField(args, "source") : selector;
                    var (sheet, addr) = SplitRange(sel);
                    return addr == "" ? null : (sheet, addr);
                }
                case "replace":
                {
                    var (sheet, addr) = SplitRange(selector);
                    if (addr != "") return (sheet, addr);
                    dynamic used = Sheet(wb, sheet).UsedRange;
                    return (sheet, (string)used.Address(false, false));
                }
                case "copy":
                {
                    var (ss, sa) = SplitRange(JsonField(args, "source"));
                    var (ds, da) = SplitRange(JsonField(args, "at"));
                    if (sa == "" || da == "") return null;
                    dynamic src = Sheet(wb, ss).Range[sa];
                    dynamic dst = Sheet(wb, ds).Range[da].Cells[1, 1];
                    return (ds, (string)dst.Resize((int)src.Rows.Count, (int)src.Columns.Count).Address(false, false));
                }
                default:
                    return null;
            }
        }

        private static long CellCount(dynamic rng) =>
            Convert.ToInt64((object)rng.CountLarge, CultureInfo.InvariantCulture);

        private static RangeSnap SnapRange(object appO, object wbO, string sheet, string addr, bool sizes)
        {
            dynamic app = appO, wb = wbO;
            dynamic ws = Sheet(wb, sheet);
            dynamic rng = ws.Range[addr];
            var snap = new RangeSnap { Sheet = sheet };
            // Whole columns are a million rows each: what they hold is the
            // part inside the used range, and that is what is copied.
            dynamic? content = rng;
            if (CellCount(rng) > UndoCellCap)
                content = app.Intersect(rng, ws.UsedRange);
            if (content != null)
            {
                long cells = CellCount(content);
                if (cells > UndoCellCap)
                    throw new InvalidOperationException($"{cells.ToString("N0", CultureInfo.InvariantCulture)} cells is more than the {UndoCellCap.ToString("N0", CultureInfo.InvariantCulture)} a call can change and still be undone");
                snap.Addr = content.Address(false, false);
                snap.Formulas = content.Formula;
                dynamic sw = Scratch(appO);
                dynamic ss = sw.Worksheets.Add();
                snap.ScratchSheet = ss.Name;
                // Values and every kind of format, comments and validation
                // come across with Copy(Destination), which never touches the
                // clipboard. The formulas are then turned into values there so
                // the scratch book holds no links into the user's workbook;
                // the formulas themselves are put back from `Formulas`.
                dynamic dest = ss.Range[snap.Addr];
                content.Copy(dest);
                dest.Value2 = content.Value2;
                sw.Saved = true;
            }
            if (sizes)
            {
                int c0 = rng.Column, nc = rng.Columns.Count, r0 = rng.Row, nr = rng.Rows.Count;
                // A whole column has no row heights worth keeping, a whole
                // row no column widths; either would be thousands of calls.
                if (nc < (int)ws.Columns.Count)
                    for (var c = c0; c < c0 + Math.Min(nc, 256); c++)
                    {
                        snap.Widths[c] = (double)ws.Columns[c].ColumnWidth;
                        snap.ColsHidden[c] = (bool)ws.Columns[c].Hidden;
                    }
                if (nr < (int)ws.Rows.Count)
                    for (var r = r0; r < r0 + Math.Min(nr, 2000); r++)
                    {
                        snap.Heights[r] = (double)ws.Rows[r].RowHeight;
                        snap.RowsHidden[r] = (bool)ws.Rows[r].Hidden;
                    }
            }
            return snap;
        }

        private static void RestoreRange(object appO, object wbO, RangeSnap snap)
        {
            dynamic wb = wbO;
            dynamic ws = Sheet(wb, snap.Sheet);
            if (snap.ScratchSheet != "")
            {
                dynamic rng = ws.Range[snap.Addr];
                dynamic sw = Scratch(appO);
                dynamic ss = sw.Worksheets[snap.ScratchSheet];
                // A merge the call made would refuse the paste ("cannot change
                // part of a merged cell"), and rules the call added would stay
                // beside the ones coming back.
                try { rng.UnMerge(); } catch { }
                try { rng.FormatConditions.Delete(); } catch { }
                ss.Range[snap.Addr].Copy(rng);
                try { rng.Formula = snap.Formulas; }
                catch (COMException) { /* merged cells take their values from the copy */ }
            }
            foreach (var (c, w) in snap.Widths) ws.Columns[c].ColumnWidth = w;
            foreach (var (c, h) in snap.ColsHidden) ws.Columns[c].Hidden = h;
            foreach (var (r, h) in snap.Heights) ws.Rows[r].RowHeight = h;
            foreach (var (r, h) in snap.RowsHidden) ws.Rows[r].Hidden = h;
        }

        private static void DropScratchSheet(object appO, string name)
        {
            if (_scratch == null || name == "") return;
            dynamic app = appO;
            try
            {
                dynamic sw = _scratch!;
                if ((int)sw.Worksheets.Count <= 1) return;
                bool alerts = app.DisplayAlerts;
                app.DisplayAlerts = false;
                try { sw.Worksheets[name].Delete(); }
                finally { app.DisplayAlerts = alerts; }
                sw.Saved = true;
            }
            catch { }
        }

        private static string CellsPrint(object wbO, RangeSnap? snap)
        {
            if (snap == null || snap.ScratchSheet == "") return "";
            dynamic wb = wbO;
            try
            {
                dynamic rng = Sheet(wb, snap.Sheet).Range[snap.Addr];
                object f = rng.Formula;
                var sb = new StringBuilder();
                if (f is object[,] a) foreach (var v in a) sb.Append(Convert.ToString(v, CultureInfo.InvariantCulture)).Append('\u0001');
                else sb.Append(Convert.ToString(f, CultureInfo.InvariantCulture));
                return Hash(sb.ToString());
            }
            catch { return "gone"; }
        }

        private static readonly string[] PageProps =
        {
            "Orientation", "PaperSize", "LeftMargin", "RightMargin", "TopMargin", "BottomMargin", "Zoom",
            "FitToPagesWide", "FitToPagesTall", "LeftHeader", "CenterHeader", "RightHeader",
            "LeftFooter", "CenterFooter", "RightFooter",
        };

        private static (string?, Func<string, string>?) ExcelBefore(object appO, string handle, string method, string line,
            string selector)
        {
            dynamic app = appO;
            dynamic? wb = FindWorkbook(app, handle);
            if (wb == null) return (null, null);
            var key = _app + ":" + (string)wb.Name;
            var args = JsonField(line, "args");
            var e = new UndoEntry { What = method + (selector != "" ? " " + selector : "") };

            if (method == "macro")
            {
                // Running VBA can do anything, and writing a module changes
                // code, not cells. `macro run` already copies the workbook
                // first; that copy is the way back.
                return (key, reply =>
                {
                    if (IsOk(reply))
                        Push(key, new UndoEntry { What = "macro", Why = "a macro can change anything; the copy it made before running (named in its reply) is the way back" });
                    return reply;
                });
            }

            // Cells the call changes, copied first.
            RangeSnap? snap = null;
            var style = method == "format" ? JsonField(line, "payload").ToLowerInvariant() : "";
            var cells = CellsOf((object)wb, method, line, selector);
            if (cells is { } c) snap = SnapRange(appO, (object)wb, c.sheet, c.addr, method == "format");
            if (snap != null) e.ScratchSheets.Add(snap.ScratchSheet);

            // Rows, columns or cells the call deletes, copied with the rows
            // around them so they can be inserted back.
            RangeSnap? deleted = null;
            string delKind = "", delAddr = "", delSheet = "";
            if (method is "delete" or "insert")
            {
                var t = Target((object)wb, selector, method);
                (delKind, delAddr, delSheet) = (t.kind, t.addr, t.sheet);
                if (method == "delete")
                {
                    dynamic whole = t.kind == "rows" ? t.rng.EntireRow : t.kind == "columns" ? t.rng.EntireColumn : t.rng;
                    deleted = SnapRange(appO, (object)wb, t.sheet, (string)whole.Address(false, false), t.kind != "cells");
                    e.ScratchSheets.Add(deleted.ScratchSheet);
                }
            }

            // A sheet the call deletes, copied whole into the scratch book.
            string sheetCopy = "", sheetName = "";
            int sheetIndex = 0;
            if (method == "sheet" && JsonField(args, "action").ToLowerInvariant() == "delete")
            {
                var (sn, _) = SplitRange(selector);
                dynamic ws = Sheet(wb, sn);
                sheetName = ws.Name;
                sheetIndex = ws.Index;
                dynamic sw = Scratch(appO);
                ws.Copy(After: sw.Worksheets[(int)sw.Worksheets.Count]);
                sheetCopy = sw.Worksheets[(int)sw.Worksheets.Count].Name;
                sw.Saved = true;
                e.ScratchSheets.Add(sheetCopy);
            }

            // Page setup the call changes.
            // Page setup the call changes: the named sheet, the active one
            // for pageSetup, every sheet for a header or page numbers.
            Dictionary<string, Dictionary<string, object>>? page = null;
            if (method is "pageSetup" or "header" or "pageNumbers")
            {
                var (sn, _) = SplitRange(selector);
                var sheets = new List<dynamic>();
                if (sn != "") sheets.Add(Sheet(wb, sn));
                else if (method == "pageSetup") sheets.Add(wb.ActiveSheet);
                else for (var i = 1; i <= (int)wb.Worksheets.Count; i++) sheets.Add(wb.Worksheets[i]);
                page = new Dictionary<string, Dictionary<string, object>>();
                foreach (var ws in sheets)
                {
                    var one = new Dictionary<string, object>();
                    dynamic ps = ws.PageSetup;
                    foreach (var p in PageProps)
                        try { one[p] = ps.GetType().InvokeMember(p, System.Reflection.BindingFlags.GetProperty, null, ps, null); } catch { }
                    page[(string)ws.Name] = one;
                }
            }

            // The window's freeze, which lives on the window, not the cells.
            (bool on, int row, int col)? freeze = null;
            if (style.Contains("freeze"))
                try { dynamic w = app.ActiveWindow; freeze = ((bool)w.FreezePanes, (int)w.SplitRow, (int)w.SplitColumn); } catch { }

            bool? filterWas = null;
            if (method == "filter" && cells is { } fc)
                filterWas = (bool)Sheet(wb, fc.sheet).AutoFilterMode;

            var before = Take((object)wb);

            return (key, reply =>
            {
                if (!IsOk(reply))
                {
                    foreach (var sh in e.ScratchSheets) DropScratchSheet(appO, sh);
                    return reply;
                }
                var after = Take((object)wb);
                var added = new Inventory();
                foreach (var s in after.Sheets.Where(s => !before.Sheets.Any(b => b.name == s.name))) added.Sheets.Add(s);
                added.Shapes.UnionWith(after.Shapes.Except(before.Shapes));
                added.Tables.UnionWith(after.Tables.Except(before.Tables));
                added.Pivots.UnionWith(after.Pivots.Except(before.Pivots));
                added.Names.UnionWith(after.Names.Except(before.Names));
                added.Slicers.UnionWith(after.Slicers.Except(before.Slicers));
                // A rename is a sheet gone and a sheet come in the same place.
                string? renamedFrom = null, renamedTo = null;
                if (method == "sheet" && JsonField(args, "action").ToLowerInvariant() == "rename")
                {
                    renamedFrom = SplitRange(selector).sheet;
                    renamedTo = JsonField(args, "name");
                    added.Sheets.RemoveAll(s => s.name == renamedTo);
                }
                var visibility = before.Sheets.Where(b => after.Sheets.Any(a => a.name == b.name && a.visible != b.visible)).ToList();

                e.Revert = d =>
                {
                    dynamic book = d;
                    bool alerts = app.DisplayAlerts;
                    app.DisplayAlerts = false;
                    try
                    {
                        foreach (var sc in added.Slicers) try { book.SlicerCaches[sc].Delete(); } catch { }
                        foreach (var p in added.Pivots)
                        {
                            var (sn, pn) = Split1(p);
                            try { book.Worksheets[sn].PivotTables(pn).TableRange2.Clear(); } catch { }
                        }
                        foreach (var t in added.Tables)
                        {
                            var (sn, tn) = Split1(t);
                            try { book.Worksheets[sn].ListObjects[tn].Unlist(); } catch { }
                        }
                        foreach (var s in added.Shapes)
                        {
                            var (sn, shn) = Split1(s);
                            try { book.Worksheets[sn].Shapes.Item(shn).Delete(); } catch { }
                        }
                        foreach (var n in added.Names) try { book.Names.Item(n).Delete(); } catch { }
                        foreach (var s in added.Sheets) try { book.Worksheets[s.name].Delete(); } catch { }
                        if (renamedTo != null) book.Worksheets[renamedTo].Name = renamedFrom;
                        foreach (var v in visibility) book.Worksheets[v.name].Visible = v.visible;
                        if (sheetCopy != "")
                        {
                            dynamic sw = Scratch(appO);
                            int count = book.Worksheets.Count;
                            if (sheetIndex <= count) sw.Worksheets[sheetCopy].Copy(Before: book.Worksheets[sheetIndex]);
                            else sw.Worksheets[sheetCopy].Copy(After: book.Worksheets[count]);
                            dynamic back = book.Worksheets[Math.Min(sheetIndex, count + 1)];
                            back.Name = sheetName;
                        }
                        if (method == "insert")
                        {
                            dynamic ws = Sheet(book, delSheet);
                            dynamic r = ws.Range[delAddr];
                            if (delKind == "rows") r.EntireRow.Delete();
                            else if (delKind == "columns") r.EntireColumn.Delete();
                            else r.Delete(-4162); // xlShiftUp, as insert shifted down
                        }
                        if (deleted != null)
                        {
                            dynamic ws = Sheet(book, delSheet);
                            dynamic r = ws.Range[delAddr];
                            if (delKind == "rows") r.EntireRow.Insert();
                            else if (delKind == "columns") r.EntireColumn.Insert();
                            else r.Insert(-4121); // xlShiftDown, as delete shifted up
                            RestoreRange(appO, (object)book, deleted);
                        }
                        if (filterWas == false && cells is { } f2) Sheet(book, f2.sheet).AutoFilterMode = false;
                        if (snap != null) RestoreRange(appO, (object)book, snap);
                        if (page != null)
                            foreach (var (sheetName2, props) in page)
                            {
                                dynamic ps = Sheet(book, sheetName2).PageSetup;
                                foreach (var (p, v) in props)
                                    try { ps.GetType().InvokeMember(p, System.Reflection.BindingFlags.SetProperty, null, ps, new[] { v }); } catch { }
                            }
                        if (freeze is { } fz)
                        {
                            dynamic w = app.ActiveWindow;
                            w.FreezePanes = false;
                            w.SplitRow = fz.row;
                            w.SplitColumn = fz.col;
                            w.FreezePanes = fz.on;
                        }
                    }
                    finally { app.DisplayAlerts = alerts; }
                    var note = method == "delete" || (method == "sheet" && sheetCopy != "")
                        ? " (formulas elsewhere that pointed into what was deleted still show #REF!)"
                        : "";
                    return e.What + note;
                };
                // Cells: their content now, to notice someone changing them
                // before the undo. Anything else is not watched.
                if (snap != null)
                {
                    var watched = snap;
                    e.Print = d => CellsPrint(d, watched);
                    e.After = e.Print((object)wb);
                }
                Push(key, e);
                return reply;
            });
        }

        private static (string, string) Split1(string s)
        {
            var i = s.IndexOf('\u0001');
            return (s[..i], s[(i + 1)..]);
        }

        // ---------------------------------------------------------- PowerPoint

        private static (string?, Func<string, string>?) PptBefore(object appO, string handle, string method, string selector)
        {
            dynamic app = appO;
            dynamic? pres = FindPresentation(app, handle);
            if (pres == null) return (null, null);
            var key = _app + ":" + (string)pres.Name;
            var dir = Path.Combine(Path.GetTempPath(), "syn-undo");
            Directory.CreateDirectory(dir);
            var ext = Path.GetExtension((string)pres.Name);
            if (string.IsNullOrEmpty(ext)) ext = ".pptx";
            var copy = Path.Combine(dir, $"{Guid.NewGuid():N}{ext}");
            // A copy of the deck as it is, unsaved changes included, without
            // moving the open deck to a new path the way SaveAs would.
            pres.SaveCopyAs(copy);
            var ids = SlideIds((object)pres);
            var prints = SlidePrints((object)pres);
            // A call that names a slide changes it even when its text does
            // not (a format, a picture).
            int named = 0;
            var m = Regex.Match(selector ?? "", @"^s(\d+)", RegexOptions.IgnoreCase);
            if (m.Success) int.TryParse(m.Groups[1].Value, out named);
            var deckWide = method is "pageNumbers";
            // The slide size lives on the deck, not on a slide.
            (double w, double h, int o)? size = null;
            if (method == "pageSetup")
            {
                dynamic ps = pres.PageSetup;
                size = ((double)ps.SlideWidth, (double)ps.SlideHeight, (int)ps.SlideOrientation);
            }

            return (key, reply =>
            {
                if (!IsOk(reply))
                {
                    try { File.Delete(copy); } catch { }
                    return reply;
                }
                var now = SlideIds((object)pres);
                var nowPrints = SlidePrints((object)pres);
                var e = new UndoEntry { What = method + (selector != "" ? " " + selector : "") };
                e.Files.Add(copy);
                var addedIds = now.Where(i => !ids.Contains(i)).ToList();
                // Slides from the copy that must go back: removed ones, and
                // ones this call changed.
                var restore = new List<(int id, int pos)>();
                for (var p = 0; p < ids.Count; p++)
                {
                    var id = ids[p];
                    var gone = !now.Contains(id);
                    var changed = !gone && (deckWide || (named == p + 1 && method is not ("duplicateSlide" or "moveSlide"))
                                            || prints[p] != nowPrints[now.IndexOf(id)]);
                    if (gone || changed) restore.Add((id, p + 1));
                }
                e.SlideIds.AddRange(ids);
                e.SlideIds.AddRange(addedIds);
                var theme = method == "theme";
                e.Revert = d =>
                {
                    dynamic deck = d;
                    // Map the ids this entry recorded to what they are now:
                    // an earlier undo may have put a slide back under a new id.
                    var order = e.SlideIds.Take(ids.Count).ToList();
                    var fresh = e.SlideIds.Skip(ids.Count).ToList();
                    foreach (var id in fresh)
                        try { deck.Slides.FindBySlideID(id).Delete(); } catch { }
                    if (theme) deck.ApplyTemplate(copy);
                    if (size is { } sz)
                    {
                        dynamic ps = deck.PageSetup;
                        ps.SlideOrientation = sz.o;
                        ps.SlideWidth = sz.w;
                        ps.SlideHeight = sz.h;
                    }
                    foreach (var (_, pos) in restore)
                    {
                        var slot = pos - 1;
                        int oldId = order[slot];
                        try { deck.Slides.FindBySlideID(oldId).Delete(); } catch { }
                        int at = Math.Min(pos - 1, (int)deck.Slides.Count);
                        deck.Slides.InsertFromFile(copy, at, pos, pos);
                        int newId = deck.Slides[at + 1].SlideID;
                        Remap(key, oldId, newId);
                        order[slot] = newId;
                    }
                    // Back in the order the copy had them.
                    for (var p = 0; p < order.Count; p++)
                        try { deck.Slides.FindBySlideID(order[p]).MoveTo(p + 1); } catch { }
                    return e.What;
                };
                e.Print = PptPrint;
                e.After = PptPrint((object)pres);
                Push(key, e);
                return reply;
            });
        }

        /// <summary>A slide put back from a copy has a new id; every entry
        /// that names the old one is rewritten to the new.</summary>
        private static void Remap(string key, int oldId, int newId)
        {
            foreach (var e in StackFor(key))
                for (var i = 0; i < e.SlideIds.Count; i++)
                    if (e.SlideIds[i] == oldId) e.SlideIds[i] = newId;
        }

        private static List<int> SlideIds(object presO)
        {
            dynamic pres = presO;
            var ids = new List<int>();
            int n = pres.Slides.Count;
            for (var i = 1; i <= n; i++) ids.Add((int)pres.Slides[i].SlideID);
            return ids;
        }

        private static List<string> SlidePrints(object presO)
        {
            dynamic pres = presO;
            var prints = new List<string>();
            int n = pres.Slides.Count;
            for (var i = 1; i <= n; i++)
            {
                dynamic slide = pres.Slides[i];
                var sb = new StringBuilder();
                int k = slide.Shapes.Count;
                sb.Append(k).Append('|');
                for (var j = 1; j <= k; j++)
                {
                    dynamic sh = slide.Shapes[j];
                    sb.Append((string)sh.Name).Append(':');
                    try
                    {
                        if ((int)sh.HasTextFrame != 0 && (int)sh.TextFrame.HasText != 0)
                            sb.Append((string)sh.TextFrame.TextRange.Text);
                    }
                    catch { }
                    sb.Append('\u0001');
                }
                try { sb.Append((string)slide.NotesPage.Shapes.Placeholders[2].TextFrame.TextRange.Text); } catch { }
                prints.Add(Hash(sb.ToString()));
            }
            return prints;
        }

        private static string PptPrint(object pres)
        {
            var ids = SlideIds(pres);
            var prints = SlidePrints(pres);
            return Hash(string.Join(",", ids.Select((id, i) => id + "=" + prints[i])));
        }

        /// <summary>Temp copies left by a helper that is going away.</summary>
        private static void DropAllUndo()
        {
            foreach (var s in Undos.Values)
                foreach (var e in s) Drop(e);
            Undos.Clear();
            CloseScratch();
        }
    }
}
