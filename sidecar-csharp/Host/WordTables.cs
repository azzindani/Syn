// Word tables as tables, and real charts from Excel.
//
// A live run spent most of its budget on one misunderstanding. Word counts
// every cell of a table as a paragraph, and one more at the end of each row,
// so `read body` showed a 12x4 table as sixty lines -- "p20: Site | p21:
// Records | ... | p24: " -- with nothing to say they were a table. The model
// took the real tables it had just built for loose paragraphs faking one,
// deleted them and built them again, three times. It then asked to format
// the tables "as tables, not one by one", and `format` took one paragraph
// at a time. And asked for the workbook's charts in the report, it could
// only export them as pictures, because there was no way to put a chart in.
//
// So a table now reads as one entry that names its paragraphs, a table or
// its rows and columns can be formatted in one call, and `embedChart`
// pastes a real chart from an open workbook, linked to it.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        /// <summary>One table: which it is (t1 first) and the paragraph
        /// numbers its cells take, the same numbers every Word selector
        /// counts in.</summary>
        private sealed class TableSpan
        {
            public int Index;
            public int First, Last;
            public int Rows, Cols;
            public object Table = null!;
        }

        private static List<TableSpan> TableSpans(object docO)
        {
            dynamic doc = docO;
            var spans = new List<TableSpan>();
            int n;
            try { n = doc.Tables.Count; } catch { return spans; }
            for (var i = 1; i <= n; i++)
            {
                dynamic t = doc.Tables[i];
                int start = t.Range.Start;
                // Paragraphs wholly before the table: its first paragraph's
                // number. A range from 0 to 0 still reports one paragraph.
                int first = start == 0 ? 0 : (int)doc.Range(0, start).Paragraphs.Count;
                int count = t.Range.Paragraphs.Count;
                int rows = 0, cols = 0;
                try { rows = t.Rows.Count; } catch { }
                try { cols = t.Columns.Count; } catch { }
                spans.Add(new TableSpan { Index = i, First = first, Last = first + count - 1, Rows = rows, Cols = cols, Table = t });
            }
            return spans;
        }

        private static TableSpan? SpanAt(List<TableSpan> spans, int para) =>
            spans.FirstOrDefault(s => para >= s.First && para <= s.Last);

        private static string CellText(string raw) =>
            raw.Replace("\r\a", "").Replace("\a", "").Replace("\r", " / ").Replace("\v", " ").Trim();

        /// <summary>A table as one line: rows by ;, cells by |, the grammar
        /// insertTable takes, so what is read can be written back.</summary>
        private static string TableEntry(TableSpan s, int cap)
        {
            dynamic t = s.Table;
            var rows = new List<string>();
            try
            {
                for (var r = 1; r <= s.Rows; r++)
                {
                    dynamic row = t.Rows[r];
                    int cells = row.Cells.Count;
                    var parts = ((string)row.Range.Text).Split("\r\a");
                    rows.Add(string.Join("|", parts.Take(cells).Select(CellText)));
                }
            }
            catch
            {
                // Vertically merged cells make Rows[r] throw. The cells are
                // still there in order; say what the shape is not.
                rows.Clear();
                rows.Add(string.Join("|", ((string)t.Range.Text).Split("\r\a").Select(CellText).Where(c => c != "")));
                rows.Add("(merged cells: shown in order, not by row)");
            }
            var head = $"p{s.First}:p{s.Last} [table t{s.Index}, {s.Rows}x{s.Cols}]: ";
            var sb = new StringBuilder(head);
            for (var i = 0; i < rows.Count; i++)
            {
                if (sb.Length + rows[i].Length > cap)
                {
                    sb.Append($"... {rows.Count - i} more row(s)");
                    break;
                }
                if (i > 0) sb.Append(';');
                sb.Append(rows[i]);
            }
            return sb.ToString();
        }

        private static readonly Regex TableSel =
            new(@"^t(\d+)(?:\.([rc])(\d+)(?::[rc]?(\d+))?)?$", RegexOptions.IgnoreCase);

        private static TableSpan TableOf(List<TableSpan> spans, int n)
        {
            if (spans.Count == 0) throw new InvalidOperationException("the document has no tables");
            if (n < 1 || n > spans.Count)
                throw new InvalidOperationException($"t{n} does not exist: the document has {spans.Count} table(s), t1 to t{spans.Count}");
            return spans[n - 1];
        }

        /// <summary>What a format call reaches: text ranges for the font and
        /// paragraph keys, shading for fill, and the table itself when the
        /// selector is a whole table.</summary>
        private sealed class WordTarget
        {
            public readonly List<object> Ranges = new();
            public readonly List<object> Shades = new();
            public object? Table;
            public string Part = "";
        }

        private static WordTarget WordTargets(object docO, string selector)
        {
            dynamic doc = docO;
            var sel = (selector ?? "").Trim();
            var target = new WordTarget();
            var tm = TableSel.Match(sel);
            if (tm.Success)
            {
                var s = TableOf(TableSpans(docO), int.Parse(tm.Groups[1].Value, CultureInfo.InvariantCulture));
                dynamic t = s.Table;
                if (!tm.Groups[2].Success)
                {
                    target.Table = t;
                    target.Ranges.Add(t.Range);
                    target.Shades.Add(t.Shading);
                    target.Part = $"table t{s.Index}";
                    return target;
                }
                var rows = tm.Groups[2].Value.Equals("r", StringComparison.OrdinalIgnoreCase);
                var a = int.Parse(tm.Groups[3].Value, CultureInfo.InvariantCulture);
                var b = tm.Groups[4].Success ? int.Parse(tm.Groups[4].Value, CultureInfo.InvariantCulture) : a;
                if (b < a) (a, b) = (b, a);
                var max = rows ? s.Rows : s.Cols;
                if (a < 1 || b > max)
                    throw new InvalidOperationException(
                        $"t{s.Index} has {(rows ? "rows" : "columns")} 1 to {max}: {(rows ? "r" : "c")}{a}"
                        + (a == b ? "" : $":{b}") + " is outside it");
                // Cell by cell rather than Rows[r]: a row with a vertically
                // merged cell cannot be taken whole, and a column never can.
                foreach (dynamic cell in t.Range.Cells)
                {
                    int at = rows ? cell.RowIndex : cell.ColumnIndex;
                    if (at < a || at > b) continue;
                    target.Ranges.Add(cell.Range);
                    target.Shades.Add(cell.Shading);
                }
                target.Part = $"t{s.Index} {(rows ? "row" : "column")}{(a == b ? $" {a}" : $"s {a} to {b}")}";
                return target;
            }
            var pm = Regex.Match(sel, @"^p(\d+)(?::p?(\d+))?$", RegexOptions.IgnoreCase);
            if (!pm.Success)
                throw new InvalidOperationException(
                    $"word format needs a paragraph like p3, a range p3:p9, or a table t2 (t2.r1 a row, t2.c3 a column), not {selector}");
            var from = int.Parse(pm.Groups[1].Value, CultureInfo.InvariantCulture);
            var to = pm.Groups[2].Success ? int.Parse(pm.Groups[2].Value, CultureInfo.InvariantCulture) : from;
            if (to < from) (from, to) = (to, from);
            int count = doc.Paragraphs.Count;
            if (to >= count)
                throw new InvalidOperationException($"the document has paragraphs p0 to p{count - 1}, not {selector}");
            dynamic range = doc.Range(doc.Paragraphs[from + 1].Range.Start, doc.Paragraphs[to + 1].Range.End);
            target.Ranges.Add(range);
            target.Shades.Add(range.ParagraphFormat.Shading);
            target.Part = from == to ? $"p{from}" : $"p{from}:p{to}";
            return target;
        }

        /// <summary>One font or paragraph key on one range; false when the
        /// key is not one of these.</summary>
        private static bool ApplyWordKey(dynamic range, string key, string val, bool on)
        {
            switch (key)
            {
                case "style": ApplyStyle(range, val); return true;
                case "bold": range.Font.Bold = on ? 1 : 0; return true;
                case "italic": range.Font.Italic = on ? 1 : 0; return true;
                case "size": range.Font.Size = double.Parse(val, CultureInfo.InvariantCulture); return true;
                case "font": range.Font.Name = val; return true;
                // wdAlignParagraphLeft 0, Center 1, Right 2, Justify 3
                case "align":
                    range.ParagraphFormat.Alignment = val.ToLowerInvariant() switch
                    {
                        "left" => 0,
                        "center" => 1,
                        "centre" => 1,
                        "right" => 2,
                        "justify" => 3,
                        _ => throw new InvalidOperationException($"align does not know {val}"),
                    };
                    return true;
                case "underline": range.Font.Underline = on ? 1 : 0; return true; // wdUnderlineSingle
                case "color": range.Font.Color = OleColor(val); return true;
                // WdColorIndex: the highlighter's own colours, by name.
                case "highlight":
                    range.HighlightColorIndex = val.ToLowerInvariant() switch
                    {
                        "yellow" => 7, "green" => 4, "cyan" or "turquoise" => 3, "pink" => 5,
                        "red" => 6, "blue" => 2, "gray" or "grey" => 16, "none" or "0" => 0,
                        _ => throw new InvalidOperationException(
                            $"highlight does not know {val}: yellow, green, cyan, pink, red, blue, gray or none"),
                    };
                    return true;
                case "spacebefore": range.ParagraphFormat.SpaceBefore = double.Parse(val, CultureInfo.InvariantCulture); return true;
                case "spaceafter": range.ParagraphFormat.SpaceAfter = double.Parse(val, CultureInfo.InvariantCulture); return true;
                // wdLineSpaceMultiple 5, in lines of 12 points: 1.5 is one
                // and a half lines whatever the font.
                case "linespacing":
                    range.ParagraphFormat.LineSpacingRule = 5;
                    range.ParagraphFormat.LineSpacing = 12.0 * double.Parse(val, CultureInfo.InvariantCulture);
                    return true;
                case "indent": range.ParagraphFormat.LeftIndent = double.Parse(val, CultureInfo.InvariantCulture); return true;
                default: return false;
            }
        }

        private const int WdColorAutomatic = -16777216;

        private static string FormatWord(dynamic doc, string handle, string selector, string styles)
        {
            Snapshot(handle);
            var target = WordTargets((object)doc, selector);
            dynamic? table = target.Table;
            var did = new List<string>();
            var notes = new List<string>();
            foreach (var pair in (styles ?? "").Split(';', StringSplitOptions.RemoveEmptyEntries))
            {
                var i = pair.IndexOf('=');
                if (i < 0) continue;
                var key = pair[..i].Trim().ToLowerInvariant();
                var val = pair[(i + 1)..].Trim();
                var on = !(val == "0" || val.Equals("false", StringComparison.OrdinalIgnoreCase));
                if (key is "tablestyle" or "banded" or "header" or "autofit" && table == null)
                    throw new InvalidOperationException(
                        $"{pair[..i].Trim()} is for a whole table: select it as t2, not {selector}");
                switch (key)
                {
                    case "tablestyle":
                        var note = ApplyStyle(table!, val);
                        if (note.Contains("left as-is")) notes.Add(note.Trim());
                        break;
                    case "banded": table!.ApplyStyleRowBands = on; break;
                    case "header": table!.ApplyStyleHeadingRows = on; break;
                    // wdAutoFitFixed 0, wdAutoFitContent 1, wdAutoFitWindow 2
                    case "autofit":
                        table!.AutoFitBehavior(val.ToLowerInvariant() switch
                        {
                            "content" or "1" => 1,
                            "window" => 2,
                            "fixed" or "0" => 0,
                            _ => throw new InvalidOperationException($"autofit is content, window or fixed, not {val}"),
                        });
                        break;
                    case "fill":
                        var colour = val.Equals("none", StringComparison.OrdinalIgnoreCase) ? WdColorAutomatic : OleColor(val);
                        foreach (dynamic s in target.Shades) s.BackgroundPatternColor = colour;
                        break;
                    default:
                        foreach (dynamic r in target.Ranges)
                        {
                            if (!ApplyWordKey(r, key, val, on))
                                throw new InvalidOperationException(
                                    $"word format does not know {key}: it knows style, bold, italic, underline, size, font, color, "
                                    + "highlight, fill, align, spaceBefore, spaceAfter, lineSpacing, indent, and on a whole table t2 "
                                    + "tableStyle, banded, header, autofit");
                        }
                        break;
                }
                did.Add(key);
            }
            doc.Saved = false;
            return Ok($"formatted {target.Part}: {string.Join(", ", did)}" + (notes.Count > 0 ? " " + string.Join(" ", notes) : ""));
        }

        // ---- embedChart ----

        private const int WdChart = 14, WdChartLinked = 15;

        private static string EmbedChart(dynamic doc, string handle, string from, string source, string at, string widthPt, string style)
        {
            // from is excel:<book>:<unit>; the runner has already checked it
            // is a handle this session was given.
            var parts = (from ?? "").Split(':');
            if (parts.Length < 2 || !parts[0].Equals("excel", StringComparison.OrdinalIgnoreCase) || parts[1] == "")
                throw new InvalidOperationException($"embedChart: from is a workbook's handle, excel:book.xlsx:workbook, not {from}");
            var book = parts[1];
            var src = (source ?? "").Trim();
            var bang = src.LastIndexOf('!');
            var sheet = (bang < 0 ? src : src[..bang]).Trim().Trim('\'');
            var n = 1;
            if (bang >= 0 && !int.TryParse(src[(bang + 1)..].Trim(), out n))
                throw new InvalidOperationException($"embedChart: source is the sheet and the chart's number on it, Summary!2, not {source}");
            if (sheet == "") throw new InvalidOperationException("embedChart: source names the sheet the chart is on, e.g. Summary!1");
            var link = true;
            foreach (var pair in (style ?? "").Split(';', StringSplitOptions.RemoveEmptyEntries))
            {
                var i = pair.IndexOf('=');
                var key = (i < 0 ? pair : pair[..i]).Trim().ToLowerInvariant();
                var val = i < 0 ? "1" : pair[(i + 1)..].Trim();
                if (key != "link") throw new InvalidOperationException($"embedChart style knows link=0 or link=1, not {pair}");
                link = Truthy(val);
            }

            // Excel as it is: the chart comes from the workbook the user has
            // open, so a missing Excel is an answer, not a reason to start one.
            dynamic xl;
            try
            {
                CLSIDFromProgID("Excel.Application", out var clsid);
                GetActiveObject(ref clsid, IntPtr.Zero, out var live);
                xl = live;
            }
            catch (COMException)
            {
                throw new InvalidOperationException($"Excel is not running, so {book} is not open: open it first");
            }
            dynamic? wb = null;
            var open = new List<string>();
            foreach (dynamic book0 in xl.Workbooks)
            {
                string name = book0.Name;
                open.Add(name);
                if (name.Equals(book, StringComparison.OrdinalIgnoreCase)) wb = book0;
            }
            if (wb == null)
                throw new InvalidOperationException(
                    $"{book} is not open in Excel" + (open.Count > 0 ? $"; open workbooks: {string.Join(", ", open)}" : ""));
            dynamic ws;
            try { ws = wb.Worksheets[sheet]; }
            catch
            {
                var names = new List<string>();
                foreach (dynamic s in wb.Worksheets) names.Add((string)s.Name);
                throw new InvalidOperationException($"{book} has no sheet {sheet}; its sheets: {string.Join(", ", names)}");
            }
            int charts = ws.ChartObjects().Count;
            if (charts == 0) throw new InvalidOperationException($"sheet {sheet} has no charts");
            if (n < 1 || n > charts)
                throw new InvalidOperationException($"sheet {sheet} has {charts} chart(s): {sheet}!1 to {sheet}!{charts}, not {sheet}!{n}");
            // A link names the workbook's file, and a workbook never saved
            // has none: the chart would point at nothing.
            if (link && string.IsNullOrEmpty((string)wb.Path))
                throw new InvalidOperationException(
                    $"{book} has never been saved, so a chart cannot link to it: save it first, or pass style link=0 to embed a copy");

            Snapshot(handle);
            dynamic co = ws.ChartObjects(n);
            string title = "";
            try { if ((bool)co.Chart.HasTitle) title = (string)co.Chart.ChartTitle.Text; } catch { }

            // The chart travels by the clipboard: Office has no other road
            // from a workbook's chart to a document's. Whatever text the user
            // had copied is put back afterwards.
            var kept = ClipboardText();
            int index;
            try
            {
                try { co.Chart.ChartArea.Copy(); }
                catch (COMException) { co.Copy(); }
                dynamic anchor;
                if (string.IsNullOrWhiteSpace(at))
                {
                    anchor = AppendParagraph(doc, "");
                    index = doc.Paragraphs.Count - 1;
                }
                else
                {
                    index = ParaBefore((object)doc, at, "embedChart");
                    anchor = ParagraphBefore(doc, index);
                }
                // An anchor made in front of a heading is a heading, and a
                // chart in a Heading 1 paragraph lands in the contents.
                ApplyStyle(anchor, "Normal");
                dynamic r = anchor.Range;
                r.Collapse(1); // wdCollapseStart: in front of the mark, keeping it
                Retry(() => r.PasteAndFormat(link ? WdChartLinked : WdChart));
            }
            finally
            {
                if (kept != null) SetClipboardText(kept);
            }

            dynamic para = doc.Paragraphs[index + 1];
            object? found = null;
            try { if ((int)para.Range.InlineShapes.Count > 0) found = para.Range.InlineShapes[1]; } catch { }
            var real = false;
            if (found != null)
            {
                dynamic shape = found;
                if (double.TryParse(widthPt, NumberStyles.Any, CultureInfo.InvariantCulture, out var width) && width > 0)
                {
                    try { shape.LockAspectRatio = -1; } catch { }
                    shape.Width = width;
                }
                // MsoTriState, not bool: -1 is true. A bool cast threw and every
                // real chart was reported as "not a chart".
                try { real = Convert.ToInt32((object)shape.HasChart) != 0; } catch { }
            }
            doc.Saved = false;
            return Ok($"chart {sheet}!{n}{(title == "" ? "" : $" \"{title}\"")} from {book} added as p{index}, "
                      + (real ? (link ? "a chart linked to the workbook" : "a chart with its own copy of the data")
                              : "as pasted (Word did not report it as a chart)")
                      + (kept == null ? "; the clipboard now holds the chart" : ""));
        }

        /// The clipboard is shared with every other program, and one busy
        /// with it refuses the next call for a moment.
        private static void Retry(Action act)
        {
            for (var attempt = 0; ; attempt++)
            {
                try { act(); return; }
                catch (COMException) when (attempt < 4) { Thread.Sleep(250); }
            }
        }

        // ---- the clipboard's text, kept and put back ----

        private const uint CfUnicodeText = 13;
        private const uint GmemMoveable = 0x0002;

        [DllImport("user32.dll", SetLastError = true)] private static extern bool OpenClipboard(IntPtr owner);
        [DllImport("user32.dll")] private static extern bool CloseClipboard();
        [DllImport("user32.dll")] private static extern bool EmptyClipboard();
        [DllImport("user32.dll")] private static extern bool IsClipboardFormatAvailable(uint format);
        [DllImport("user32.dll")] private static extern IntPtr GetClipboardData(uint format);
        [DllImport("user32.dll")] private static extern IntPtr SetClipboardData(uint format, IntPtr mem);
        [DllImport("kernel32.dll")] private static extern IntPtr GlobalLock(IntPtr mem);
        [DllImport("kernel32.dll")] private static extern bool GlobalUnlock(IntPtr mem);
        [DllImport("kernel32.dll")] private static extern IntPtr GlobalAlloc(uint flags, UIntPtr bytes);
        [DllImport("kernel32.dll")] private static extern IntPtr GlobalFree(IntPtr mem);

        private static bool OpenClipboardSoon()
        {
            for (var i = 0; i < 10; i++)
            {
                if (OpenClipboard(IntPtr.Zero)) return true;
                Thread.Sleep(50);
            }
            return false;
        }

        private static string? ClipboardText()
        {
            try
            {
                if (!IsClipboardFormatAvailable(CfUnicodeText) || !OpenClipboardSoon()) return null;
                try
                {
                    var h = GetClipboardData(CfUnicodeText);
                    if (h == IntPtr.Zero) return null;
                    var p = GlobalLock(h);
                    if (p == IntPtr.Zero) return null;
                    try { return Marshal.PtrToStringUni(p); }
                    finally { GlobalUnlock(h); }
                }
                finally { CloseClipboard(); }
            }
            catch { return null; }
        }

        private static void SetClipboardText(string text)
        {
            try
            {
                if (!OpenClipboardSoon()) return;
                try
                {
                    EmptyClipboard();
                    var bytes = (text.Length + 1) * 2;
                    var h = GlobalAlloc(GmemMoveable, (UIntPtr)bytes);
                    if (h == IntPtr.Zero) return;
                    var p = GlobalLock(h);
                    if (p == IntPtr.Zero) { GlobalFree(h); return; }
                    try
                    {
                        Marshal.Copy(text.ToCharArray(), 0, p, text.Length);
                        Marshal.WriteInt16(p, text.Length * 2, 0);
                    }
                    finally { GlobalUnlock(h); }
                    // The clipboard owns the memory once this succeeds.
                    if (SetClipboardData(CfUnicodeText, h) == IntPtr.Zero) GlobalFree(h);
                }
                finally { CloseClipboard(); }
            }
            catch { /* the chart is in; a clipboard we could not restore is not worth failing it */ }
        }
    }
}
