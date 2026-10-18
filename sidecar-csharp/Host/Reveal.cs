// Showing the person what Syn just changed in Excel.
//
// Syn writes through COM, which never moves the window: a column filled at
// row 50,000, a chart put at S3, a result on another sheet all happened out of
// sight, and a person watching (or a recording of the run) saw the cells they
// were already looking at: the work looked like it ran in the background and
// appeared. After a call that changed something, the window goes to it: the
// sheet is brought up, what was made (the table, the pivot, the chart, the
// slicer) is scrolled to the middle of the window and selected, so it is the
// thing on screen.

using System;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        /// <summary>Where a successful Excel call put its change, as
        /// `Sheet!A1:B2` (or just a sheet name, for a sheet); empty when the
        /// call changed nothing a person could look at (a save, a macro).</summary>
        private static string RevealTarget(string method, string selector, string args) => method switch
        {
            "write" or "format" or "conditional" or "sort" or "filter" or "dedupe" or "validate" or "comment"
                or "link" or "replace" or "insert" or "delete" or "values" or "picture" or "header" => selector,
            "chart" or "pivot" or "copy" or "slicer" or "name" => JsonField(args, "at"),
            "table" => JsonField(args, "source"),
            "addSheet" => JsonField(args, "name") is { Length: > 0 } n ? n + "!A1" : "",
            // The sheet an action ended on; deleting or hiding one leaves
            // nothing to look at.
            "sheet" => JsonField(args, "action").ToLowerInvariant() switch
            {
                "rename" or "copy" => (JsonField(args, "name") is { Length: > 0 } nn ? nn : "") + "!A1",
                "move" or "show" => SplitRange(selector).Item1 + "!A1",
                _ => "",
            },
            _ => "",
        };

        /// <summary>The paragraph or table a Word selector (`p3`, `t2`, `p3:p5`)
        /// names, or null. Paragraph selectors count from zero, tables from
        /// one, as every other Word call here does.</summary>
        private static dynamic? WordRangeOf(dynamic doc, string selector)
        {
            var m = System.Text.RegularExpressions.Regex.Match((selector ?? "").Trim(), @"^([pt])(\d+)");
            if (!m.Success) return null;
            int n = int.Parse(m.Groups[2].Value);
            return m.Groups[1].Value == "p" ? doc.Paragraphs[n + 1].Range : doc.Tables[n].Range;
        }

        /// <summary>Put what Syn just wrote in a Word document in view: the
        /// paragraph or table it made or changed is scrolled to near the top
        /// of the window (a third of the way down, so what is above it still
        /// shows) and selected. Best effort and silent, like Reveal.</summary>
        private static void RevealWord(dynamic app, dynamic doc, string method, string selector, string args)
        {
            try
            {
                dynamic? r = null;
                var at = JsonField(args, "at").Trim();
                switch (method)
                {
                    case "insertParagraph":
                        r = at.Length == 0 ? doc.Paragraphs[doc.Paragraphs.Count].Range
                                           : doc.Paragraphs[int.Parse(at[1..]) + 1].Range;
                        break;
                    case "insertTable":
                        r = at.Length == 0 ? doc.Tables[doc.Tables.Count].Range
                                           : doc.Paragraphs[int.Parse(at[1..]) + 1].Range;
                        break;
                    case "write" or "format" or "comment" or "link" or "delete":
                        r = WordRangeOf(doc, selector);
                        break;
                }
                if (r is null) return;
                try { if ((string)app.ActiveDocument.Name != (string)doc.Name) doc.Activate(); } catch (Exception) { }
                dynamic win = app.ActiveWindow;
                if (win is null) return;
                r.Select();
                win.ScrollIntoView(r, true);
                try { win.SmallScroll(Type.Missing, 8); } catch (Exception) { }
            }
            catch (Exception e) { Trace($"reveal word: {e.Message}"); }
        }

        /// <summary>Take the PowerPoint window to the slide Syn just made or
        /// changed. A new slide is the last; a selector (`s3`) names the one
        /// it changed; a moved or duplicated slide is shown where it landed.</summary>
        private static void RevealPpt(dynamic app, dynamic pres, string method, string selector, string args)
        {
            try
            {
                int n = 0;
                var sel = (selector ?? "").Trim();
                var m = System.Text.RegularExpressions.Regex.Match(sel, @"^s(\d+)", System.Text.RegularExpressions.RegexOptions.IgnoreCase);
                switch (method)
                {
                    case "createSlide": n = (int)pres.Slides.Count; break;
                    case "duplicateSlide": if (m.Success) n = int.Parse(m.Groups[1].Value) + 1; break;
                    case "moveSlide":
                    {
                        var to = System.Text.RegularExpressions.Regex.Match(JsonField(args, "at"), @"(\d+)");
                        if (to.Success) n = int.Parse(to.Groups[1].Value);
                        break;
                    }
                    case "write" or "format" or "insertTable" or "picture" or "textBox":
                        if (m.Success) n = int.Parse(m.Groups[1].Value);
                        break;
                }
                int count = (int)pres.Slides.Count;
                if (n < 1 || n > count) return;
                try { if ((string)app.ActivePresentation.Name != (string)pres.Name) pres.Windows[1].Activate(); } catch (Exception) { }
                app.ActiveWindow.View.GotoSlide(n);
            }
            catch (Exception e) { Trace($"reveal ppt: {e.Message}"); }
        }

        /// <summary>What a change covers on the sheet, as a range, so it can
        /// be put in the middle of the window whole. A drawing (a chart, a
        /// slicer, a picture) whose top-left corner is on the cell is the
        /// whole of the cells it spans. A single cell with data around it is
        /// the block it belongs to (a pivot, a table, a note under one), unless
        /// that block is large, where the cell itself is what was changed.</summary>
        private static dynamic ExtentOf(dynamic ws, dynamic r, out dynamic? shape)
        {
            shape = null;
            int row = (int)r.Row, col = (int)r.Column;
            try
            {
                int n = (int)ws.Shapes.Count;
                for (var i = 1; i <= n && i <= 300; i++)
                {
                    dynamic s = ws.Shapes.Item(i);
                    dynamic tl = s.TopLeftCell;
                    if ((int)tl.Row == row && (int)tl.Column == col)
                    {
                        shape = s;
                        return ws.Range[tl, s.BottomRightCell];
                    }
                }
            }
            catch (Exception e) { Trace($"reveal: shapes: {e.Message}"); }
            try
            {
                if ((long)r.Rows.Count * (long)r.Columns.Count == 1)
                {
                    dynamic region = r.CurrentRegion;
                    if ((long)region.Rows.Count * (long)region.Columns.Count <= 5000) return region;
                }
            }
            catch (Exception e) { Trace($"reveal: region: {e.Message}"); }
            return r;
        }

        /// <summary>Bring the changed cells into the middle of the window.
        /// Best effort and silent: a window that is hidden, minimized or busy
        /// makes it do nothing, and the call it follows has already succeeded.
        /// A range taller or wider than the window is put at the top or left
        /// edge instead of centered, and one inside frozen rows or columns is
        /// already on screen on that axis.</summary>
        private static void Reveal(dynamic app, dynamic wb, string method, string selector, string args)
        {
            try
            {
                var target = RevealTarget(method, selector, args);
                if (target.Length == 0) return;
                var (sheet, addr) = SplitRange(target);
                if (string.IsNullOrEmpty(addr)) addr = "A1";
                dynamic ws = Sheet(wb, sheet);
                // The workbook's own window first, when more than one is open.
                try { if ((string)app.ActiveWorkbook.Name != (string)wb.Name) wb.Activate(); } catch (Exception) { }
                ws.Activate();
                dynamic win = app.ActiveWindow;
                if (win is null) return;
                dynamic r = ExtentOf(ws, RangeOf((object)ws, addr), out dynamic? shape);
                int rows = (int)win.VisibleRange.Rows.Count, cols = (int)win.VisibleRange.Columns.Count;
                int r1 = (int)r.Row, c1 = (int)r.Column;
                int rn = Math.Min((int)r.Rows.Count, rows), cn = Math.Min((int)r.Columns.Count, cols);
                int top = Math.Max(1, r1 - (rows - rn) / 2), left = Math.Max(1, c1 - (cols - cn) / 2);
                bool frozen = (bool)win.FreezePanes;
                if (!(frozen && r1 <= (int)win.SplitRow)) win.ScrollRow = top;
                if (!(frozen && c1 <= (int)win.SplitColumn)) win.ScrollColumn = left;
                // Selected, so it shows which one it is: a drawing by its
                // handles, a range by its outline; a big range only by its
                // first cell, or the whole column lights up.
                if (shape is not null) { try { shape.Select(); return; } catch (Exception) { } }
                long span = (long)r.Rows.Count * (long)r.Columns.Count;
                if (span <= 2000) r.Select(); else r.Cells[1, 1].Select();
            }
            catch (Exception e) { Trace($"reveal: {e.Message}"); }
        }
    }
}
