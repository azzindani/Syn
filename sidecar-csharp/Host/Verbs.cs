// The verbs that take office-host from "fills in a document" to "does what
// a person at the keyboard does": find and replace, delete and insert, sort
// and filter, sheets, validation, comments, links, page setup, headers,
// text boxes, slide order, themes.
//
// Each is one method behind one case in the method switches in Program.cs,
// and each takes exactly the fields `tools::OFFICE_VERBS` says it does.
// Same rules as the rest of this helper: every call runs on the STA thread
// under Guarded, writes take a Snapshot first, nothing here saves, closes or
// quits anything, and a refusal says what to send instead.
//
// STATUS: compiles (net8.0-windows). NOT yet run against live Office: the
// COM calls follow the documented object models, but every one of them
// needs its first run on Windows (scripts/live-office-peak.ps1 does that).
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text.RegularExpressions;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        // Word and Excel both stop Find and Replace at 255 characters, and
        // both say so with an error that names neither.
        private const int FindCap = 255;
        // How many places `find` lists before it says "and more".
        private const int FindShown = 20;

        // ================================================================ Excel

        /// A range and what shape it is: whole rows ("5:7"), whole columns
        /// ("C:E"), or a block of cells. Insert and delete behave differently
        /// for each, and a model should not have to know Excel's EntireRow.
        // Helpers that hand back something typed take the workbook, document
        // or deck as `object` and are called with an `(object)` cast: a call
        // with a `dynamic` argument is bound at run time, and its result
        // comes back `dynamic` -- a tuple's field names are gone by then.
        private static (dynamic ws, dynamic rng, string kind, string sheet, string addr) Target(
            object wbObj, string selector, string verb)
        {
            dynamic wb = wbObj;
            var (sheet, addr) = SplitRange(selector);
            if (string.IsNullOrEmpty(addr))
                throw new InvalidOperationException(
                    $"{verb} needs a range like Sheet!A1:D20, rows like Sheet!5:7 or columns like Sheet!C:E");
            var kind = "cells";
            if (Regex.IsMatch(addr, @"^\$?\d+(:\$?\d+)?$"))
            {
                kind = "rows";
                if (!addr.Contains(':')) addr = $"{addr}:{addr}";
            }
            else if (Regex.IsMatch(addr, @"^\$?[A-Za-z]{1,3}(:\$?[A-Za-z]{1,3})?$"))
            {
                kind = "columns";
                if (!addr.Contains(':')) addr = $"{addr}:{addr}";
            }
            dynamic ws = Sheet(wb, sheet);
            return (ws, ws.Range[addr], kind, sheet, addr);
        }

        /// Quote a sheet name the way a selector needs it.
        private static string SheetRef(string name) =>
            name.Contains(' ') || name.Contains('\'') ? $"'{name.Replace("'", "''")}'" : name;

        /// The column of `rng` whose first-row header is `header`, one-based.
        /// A header, not a letter, because that is what a model reads back.
        private static int HeaderColumn(object rngObj, string header, string verb)
        {
            dynamic rng = rngObj;
            int n = rng.Columns.Count;
            var names = new List<string>();
            for (var j = 1; j <= n; j++)
            {
                object v = rng.Cells[1, j].Value2;
                var h = Convert.ToString(v, CultureInfo.InvariantCulture) ?? "";
                if (string.Equals(h.Trim(), header.Trim(), StringComparison.OrdinalIgnoreCase)) return j;
                names.Add(h);
            }
            throw new InvalidOperationException(
                $"{verb}: no column headed {header} in the first row of the range; its headers are {string.Join(", ", names)}");
        }

        private static string ExcelInsert(dynamic wb, string handle, string selector)
        {
            Snapshot(handle);
            var t = Target((object)wb, selector, "insert");
            const int xlShiftDown = -4121;
            if (t.kind == "rows") t.rng.EntireRow.Insert();
            else if (t.kind == "columns") t.rng.EntireColumn.Insert();
            else t.rng.Insert(xlShiftDown);
            return Ok($"inserted {t.kind} at {SheetRef(t.sheet)}!{t.addr}; what was there moved {(t.kind == "columns" ? "right" : "down")}");
        }

        private static string ExcelDelete(dynamic wb, string handle, string selector)
        {
            Snapshot(handle);
            var t = Target((object)wb, selector, "delete");
            const int xlShiftUp = -4162;
            if (t.kind == "rows") t.rng.EntireRow.Delete();
            else if (t.kind == "columns") t.rng.EntireColumn.Delete();
            else t.rng.Delete(xlShiftUp);
            return Ok($"deleted {t.kind} {SheetRef(t.sheet)}!{t.addr}; what was after them moved {(t.kind == "columns" ? "left" : "up")}");
        }

        private static string ExcelSort(dynamic wb, string handle, string selector, string header, string order)
        {
            Snapshot(handle);
            var t = Target((object)wb, selector, "sort");
            int col = HeaderColumn((object)t.rng, header, "sort");
            var desc = (order ?? "").Trim().ToLowerInvariant() switch
            {
                "" or "asc" or "ascending" => false,
                "desc" or "descending" => true,
                _ => throw new InvalidOperationException($"sort: rule is asc or desc, not {order}"),
            };
            // xlAscending 1, xlDescending 2; Header xlYes 1: the first row
            // stays where it is.
            t.rng.Sort(Key1: t.rng.Columns[col], Order1: desc ? 2 : 1, Header: 1);
            return Ok($"sorted {SheetRef(t.sheet)}!{t.addr} by {header}, {(desc ? "largest" : "smallest")} first");
        }

        private static string ExcelFilter(dynamic wb, string handle, string selector, string header, string rule)
        {
            Snapshot(handle);
            var t = Target((object)wb, selector, "filter");
            if (string.IsNullOrWhiteSpace(rule))
            {
                if ((bool)t.ws.AutoFilterMode) t.ws.AutoFilterMode = false;
                return Ok($"filter cleared on {SheetRef(t.sheet)}: every row shows");
            }
            int col = HeaderColumn((object)t.rng, header, "filter");
            // An AutoFilter already on another range would take this one's
            // criteria against the wrong columns.
            if ((bool)t.ws.AutoFilterMode && (string)t.ws.AutoFilter.Range.Address(false, false) != (string)t.rng.Address(false, false))
                t.ws.AutoFilterMode = false;
            var crit = rule.Trim();
            if (!(crit.StartsWith("=") || crit.StartsWith(">") || crit.StartsWith("<"))) crit = "=" + crit;
            t.rng.AutoFilter(Field: col, Criteria1: crit);
            var shown = "some";
            try
            {
                // xlCellTypeVisible 12; the header row is one of them.
                int visible = t.rng.Columns[1].SpecialCells(12).Count;
                shown = (visible - 1).ToString(CultureInfo.InvariantCulture);
            }
            catch (COMException) { shown = "0"; }
            return Ok($"filtered {SheetRef(t.sheet)}!{t.addr} to {header} {crit}: {shown} row(s) showing");
        }

        private static string ExcelDedupe(dynamic wb, string handle, string selector, string headers)
        {
            Snapshot(handle);
            var t = Target((object)wb, selector, "dedupe");
            int n = t.rng.Columns.Count;
            int before = t.rng.Rows.Count - 1;
            var cols = string.IsNullOrWhiteSpace(headers)
                ? Enumerable.Range(1, n).ToArray()
                : headers.Split('|').Select(h => HeaderColumn((object)t.rng, h, "dedupe")).ToArray();
            // A VARIANT array of VARIANTs is what RemoveDuplicates takes; an
            // int[] marshals as something else and is refused.
            object colArr = cols.Select(c => (object)c).ToArray();
            t.rng.RemoveDuplicates(colArr, 1);
            double left = t.ws.Application.WorksheetFunction.CountA(t.rng.Columns[1]);
            return Ok($"removed duplicate rows from {SheetRef(t.sheet)}!{t.addr}: {before} data row(s) before, {(int)left - 1} after");
        }

        private static string ExcelCopy(dynamic wb, string handle, string source, string at)
        {
            Snapshot(handle);
            var (ss, sa) = SplitRange(source);
            var (ds, da) = SplitRange(at);
            if (string.IsNullOrEmpty(sa)) throw new InvalidOperationException("copy needs a source like data!A1:D20");
            if (string.IsNullOrEmpty(da)) throw new InvalidOperationException("copy needs a destination cell like Summary!A1");
            dynamic src = Sheet(wb, ss).Range[sa];
            dynamic dst = Sheet(wb, ds).Range[da];
            // Copy with a destination goes cell to cell and never touches the
            // clipboard, which is the human's.
            src.Copy(dst.Cells[1, 1]);
            return Ok($"copied {SheetRef(ss)}!{sa} to {SheetRef(ds)}!{da} ({src.Rows.Count}x{src.Columns.Count}: values, formulas and formats)");
        }

        private static string ExcelValidate(dynamic wb, string handle, string selector, string rule)
        {
            Snapshot(handle);
            var t = Target((object)wb, selector, "validate");
            var i = (rule ?? "").IndexOf('=');
            if (i < 0) throw new InvalidOperationException("validate: rule is list=a,b,c, whole=1..10 or decimal=0..1");
            var kind = rule![..i].Trim().ToLowerInvariant();
            var val = rule[(i + 1)..].Trim();
            dynamic v = t.rng.Validation;
            v.Delete();
            const int alertStop = 1, between = 1;
            switch (kind)
            {
                case "list":
                {
                    // A range reference passes through; a written list is joined
                    // with the list separator, which is a comma in English Excel
                    // and a semicolon in much of the world. Try one, then the
                    // other, rather than guess the machine's locale.
                    if (val.StartsWith("="))
                    {
                        v.Add(3, alertStop, between, val);
                    }
                    else
                    {
                        var items = val.Split(',').Select(x => x.Trim()).Where(x => x.Length > 0).ToArray();
                        if (items.Length == 0) throw new InvalidOperationException("validate: list= needs its choices, like list=Yes,No");
                        try { v.Add(3, alertStop, between, string.Join(",", items)); }
                        catch (COMException) { v.Add(3, alertStop, between, string.Join(";", items)); }
                    }
                    v.InCellDropdown = true;
                    break;
                }
                case "whole":
                case "decimal":
                {
                    var parts = val.Split("..");
                    if (parts.Length != 2
                        || !double.TryParse(parts[0].Trim(), NumberStyles.Float, CultureInfo.InvariantCulture, out var lo)
                        || !double.TryParse(parts[1].Trim(), NumberStyles.Float, CultureInfo.InvariantCulture, out var hi))
                        throw new InvalidOperationException($"validate: {kind}= takes a range like {kind}=1..10");
                    // Numbers, not text: text would be read in the machine's
                    // locale, where 0.5 can mean five tenths or nothing at all.
                    v.Add(kind == "whole" ? 1 : 2, alertStop, between, lo, hi);
                    break;
                }
                default:
                    throw new InvalidOperationException($"validate does not know {kind}: it takes list, whole or decimal");
            }
            v.IgnoreBlank = true;
            return Ok($"{SheetRef(t.sheet)}!{t.addr} now takes only {kind} {val}");
        }

        private static string ExcelSheet(dynamic wb, string handle, string selector, string action, string name)
        {
            Snapshot(handle);
            var (sheetName, _) = SplitRange(selector);
            dynamic ws = Sheet(wb, sheetName);
            switch ((action ?? "").ToLowerInvariant())
            {
                case "rename":
                    ws.Name = name;
                    return Ok($"sheet {sheetName} is now {name}; selectors say {SheetRef(name)}!A1");
                case "delete":
                {
                    if ((int)wb.Worksheets.Count == 1)
                        throw new InvalidOperationException("a workbook keeps at least one sheet: add another before deleting this one");
                    // Delete asks first when alerts are on. Off for this one
                    // call, then back to whatever they were.
                    dynamic app = wb.Application;
                    bool alerts = app.DisplayAlerts;
                    app.DisplayAlerts = false;
                    try { ws.Delete(); }
                    finally { app.DisplayAlerts = alerts; }
                    return Ok($"sheet {sheetName} deleted");
                }
                case "copy":
                {
                    ws.Copy(After: ws);
                    dynamic copy = wb.Worksheets[(int)ws.Index + 1];
                    copy.Name = name;
                    return Ok($"sheet {sheetName} copied as {name}");
                }
                case "hide":
                    ws.Visible = 0; // xlSheetHidden
                    return Ok($"sheet {sheetName} hidden");
                case "show":
                    ws.Visible = -1; // xlSheetVisible
                    return Ok($"sheet {sheetName} shown");
                default:
                    throw new InvalidOperationException($"sheet does not know {action}: rename, delete, copy, hide or show");
            }
        }

        private static string ExcelComment(dynamic wb, string handle, string selector, string text)
        {
            Snapshot(handle);
            var (sheet, addr) = SplitRange(selector);
            if (string.IsNullOrEmpty(addr)) throw new InvalidOperationException("comment needs a cell like data!B2");
            dynamic cell = Sheet(wb, sheet).Range[addr].Cells[1, 1];
            // A note, the kind every Excel has. One per cell: a second
            // replaces the first rather than failing.
            cell.ClearComments();
            cell.AddComment(text ?? "");
            return Ok($"note on {SheetRef(sheet)}!{(string)cell.Address(false, false)}");
        }

        private static string ExcelLink(dynamic wb, string handle, string selector, string url, string title)
        {
            Snapshot(handle);
            var (sheet, addr) = SplitRange(selector);
            if (string.IsNullOrEmpty(addr)) throw new InvalidOperationException("link needs a cell like data!B2");
            dynamic ws = Sheet(wb, sheet);
            dynamic cell = ws.Range[addr].Cells[1, 1];
            if (string.IsNullOrWhiteSpace(title)) ws.Hyperlinks.Add(Anchor: cell, Address: url);
            else ws.Hyperlinks.Add(Anchor: cell, Address: url, TextToDisplay: title);
            return Ok($"link on {SheetRef(sheet)}!{(string)cell.Address(false, false)} to {url}");
        }

        private static string ExcelPageSetup(dynamic wb, string handle, string selector, string style)
        {
            Snapshot(handle);
            var (sheet, _) = SplitRange(selector ?? "");
            dynamic ws = string.IsNullOrEmpty(sheet) ? wb.ActiveSheet : Sheet(wb, sheet);
            dynamic ps = ws.PageSetup;
            var did = new List<string>();
            foreach (var (k, v) in Pairs(style))
            {
                switch (k)
                {
                    case "orientation":
                        ps.Orientation = v.StartsWith("land", StringComparison.OrdinalIgnoreCase) ? 2 : 1;
                        break;
                    case "paper":
                        // xlPaperLetter 1, xlPaperLegal 5, xlPaperA3 8, xlPaperA4 9
                        ps.PaperSize = v.ToUpperInvariant() switch
                        {
                            "A4" => 9, "A3" => 8, "LETTER" => 1, "LEGAL" => 5,
                            _ => throw new InvalidOperationException($"paper does not know {v}: A4, A3, Letter or Legal"),
                        };
                        break;
                    case "margin":
                    {
                        var pts = Inches(v) * 72.0;
                        ps.LeftMargin = pts; ps.RightMargin = pts; ps.TopMargin = pts; ps.BottomMargin = pts;
                        break;
                    }
                    case "fitwide":
                    case "fittall":
                    {
                        // Fit-to-page only counts once zoom is off; 0 means
                        // "as many as it takes" in that direction.
                        ps.Zoom = false;
                        var n = int.Parse(v, CultureInfo.InvariantCulture);
                        if (k == "fitwide") { if (n == 0) ps.FitToPagesWide = false; else ps.FitToPagesWide = n; }
                        else { if (n == 0) ps.FitToPagesTall = false; else ps.FitToPagesTall = n; }
                        break;
                    }
                    default:
                        throw new InvalidOperationException(
                            $"pageSetup does not know {k}: orientation, paper, margin, fitWide, fitTall");
                }
                did.Add(k);
            }
            if (did.Count == 0) throw new InvalidOperationException("pageSetup was given nothing: try orientation=landscape;fitWide=1;fitTall=0");
            return Ok($"page setup for {(string)ws.Name}: {string.Join(", ", did)}");
        }

        private static string SheetPicture(dynamic wb, string handle, string selector, string path)
        {
            Snapshot(handle);
            var full = Path.GetFullPath(path);
            if (!File.Exists(full)) throw new InvalidOperationException($"no picture at {full}");
            var (sheet, addr) = SplitRange(selector ?? "");
            if (string.IsNullOrEmpty(addr))
                throw new InvalidOperationException("a picture on a sheet needs the range it fills in `selector`, like Dashboard!A1:H16");
            dynamic ws = Sheet(wb, sheet);
            dynamic box = ws.Range[addr];
            // msoFalse: embed, do not link; msoTrue: save it with the workbook.
            ws.Shapes.AddPicture(full, 0, -1, box.Left, box.Top, box.Width, box.Height);
            return Ok($"picture on {SheetRef(sheet)}!{addr}");
        }

        private static string ExcelFind(dynamic wb, string selector, string text)
        {
            CheckFindable(text, "find");
            var hits = new List<string>();
            var more = false;
            foreach (var (ws, rng) in SearchRanges((object)wb, selector))
            {
                string name = ws.Name;
                // xlValues -4163, xlPart 2: what the cell shows, anywhere in it.
                dynamic first = rng.Find(What: text, LookIn: -4163, LookAt: 2, MatchCase: false);
                if (first == null) continue;
                string firstAddr = first.Address(false, false);
                dynamic cur = first;
                string a = firstAddr;
                do
                {
                    if (hits.Count == FindShown) { more = true; break; }
                    hits.Add($"{SheetRef(name)}!{a}");
                    cur = rng.FindNext(cur);
                    if (cur == null) break;
                    a = cur.Address(false, false);
                } while (a != firstAddr);
                if (more) break;
            }
            if (hits.Count == 0) return Ok($"no cell shows {text}");
            return Ok($"{text} is in {hits.Count}{(more ? "+" : "")} cell(s): {string.Join(", ", hits)}");
        }

        private static string ExcelReplace(dynamic wb, string handle, string selector, string text, string with)
        {
            CheckFindable(text, "replace");
            CheckFindable(with, "replace");
            Snapshot(handle);
            var count = 0;
            foreach (var (_, rng) in SearchRanges((object)wb, selector))
            {
                dynamic first = rng.Find(What: text, LookIn: -4123, LookAt: 2, MatchCase: false); // xlFormulas
                if (first == null) continue;
                string firstAddr = first.Address(false, false);
                dynamic cur = first;
                while (true)
                {
                    count++;
                    cur = rng.FindNext(cur);
                    if (cur == null || count >= 100000) break;
                    string a = cur!.Address(false, false);
                    if (a == firstAddr) break;
                }
                rng.Replace(What: text, Replacement: with, LookAt: 2, MatchCase: false);
            }
            return Ok(count == 0 ? $"no cell contains {text}; nothing changed" : $"replaced {text} with {with} in {count} cell(s)");
        }

        /// A selector for find and replace: a sheet, a range, or nothing for
        /// every sheet. Only the used part of a sheet is searched.
        private static IEnumerable<(dynamic ws, dynamic rng)> SearchRanges(object wbObj, string selector)
        {
            dynamic wb = wbObj;
            var (sheet, addr) = SplitRange(selector ?? "");
            if (string.IsNullOrEmpty(sheet))
            {
                int n = wb.Worksheets.Count;
                for (var i = 1; i <= n; i++)
                {
                    dynamic ws = wb.Worksheets[i];
                    yield return (ws, ws.UsedRange);
                }
                yield break;
            }
            dynamic one = Sheet(wb, sheet);
            yield return (one, string.IsNullOrEmpty(addr) ? one.UsedRange : one.Range[addr]);
        }

        private static void CheckFindable(string text, string verb)
        {
            if (string.IsNullOrEmpty(text)) throw new InvalidOperationException($"{verb} needs the text to look for in `text`");
            if (text.Length > FindCap)
                throw new InvalidOperationException($"{verb} works on up to {FindCap} characters at a time, and this is {text.Length}");
        }

        private static IEnumerable<(string k, string v)> Pairs(string style)
        {
            foreach (var pair in (style ?? "").Split(';', StringSplitOptions.RemoveEmptyEntries))
            {
                var i = pair.IndexOf('=');
                if (i < 0) continue;
                yield return (pair[..i].Trim().ToLowerInvariant(), pair[(i + 1)..].Trim());
            }
        }

        private static double Inches(string v) =>
            double.TryParse(v.Trim().TrimEnd('"').Replace("in", ""), NumberStyles.Float, CultureInfo.InvariantCulture, out var x)
                ? x
                : throw new InvalidOperationException($"margin is in inches, like margin=0.75, not {v}");

        // ================================================================= Word

        /// "p3" or "p3:p5", zero-based, checked against the document.
        private static (int a, int b) ParaSpan(object docObj, string selector, string verb)
        {
            dynamic doc = docObj;
            var m = Regex.Match((selector ?? "").Trim(), @"^p(\d+)(?::p?(\d+))?$", RegexOptions.IgnoreCase);
            if (!m.Success) throw new InvalidOperationException($"{verb} needs a paragraph like p3, or p3:p5 for several");
            var a = int.Parse(m.Groups[1].Value, CultureInfo.InvariantCulture);
            var b = m.Groups[2].Success ? int.Parse(m.Groups[2].Value, CultureInfo.InvariantCulture) : a;
            int count = doc.Paragraphs.Count;
            if (b < a || b >= count)
                throw new InvalidOperationException($"{verb}: the document has paragraphs p0 to p{count - 1}");
            return (a, b);
        }

        private static string WordDelete(dynamic doc, string handle, string selector)
        {
            Snapshot(handle);
            var (a, b) = ParaSpan((object)doc, selector, "delete");
            dynamic r = doc.Range(doc.Paragraphs[a + 1].Range.Start, doc.Paragraphs[b + 1].Range.End);
            r.Delete();
            doc.Saved = false;
            return Ok($"deleted {(a == b ? $"p{a}" : $"p{a} to p{b}")}; the paragraphs after moved up, so p{a} is now what followed");
        }

        private static string WordFind(dynamic doc, string text)
        {
            CheckFindable(text, "find");
            var hits = new List<string>();
            var more = false;
            dynamic rng = doc.Content;
            dynamic f = rng.Find;
            f.ClearFormatting();
            // wdFindStop 0: one pass, top to bottom.
            while ((bool)f.Execute(FindText: text, MatchCase: false, MatchWholeWord: false,
                                   MatchWildcards: false, Forward: true, Wrap: 0))
            {
                if (hits.Count == FindShown) { more = true; break; }
                // Paragraphs from the top to the end of the hit, counted:
                // one past the hit's zero-based paragraph index.
                int idx = doc.Range(0, rng.End).Paragraphs.Count - 1;
                var p = $"p{idx}";
                if (!hits.Contains(p)) hits.Add(p);
                rng.Collapse(0); // wdCollapseEnd
            }
            if (hits.Count == 0) return Ok($"{text} does not appear in the document");
            return Ok($"{text} appears in {hits.Count}{(more ? "+" : "")} paragraph(s): {string.Join(", ", hits)}");
        }

        private static string WordReplace(dynamic doc, string handle, string text, string with)
        {
            CheckFindable(text, "replace");
            CheckFindable(with, "replace");
            Snapshot(handle);
            var count = 0;
            dynamic probe = doc.Content;
            dynamic pf = probe.Find;
            pf.ClearFormatting();
            while (count < 100000 && (bool)pf.Execute(FindText: text, MatchCase: false, MatchWholeWord: false,
                                                      MatchWildcards: false, Forward: true, Wrap: 0))
            {
                count++;
                probe.Collapse(0);
            }
            if (count == 0) return Ok($"{text} does not appear in the document; nothing changed");
            dynamic rng = doc.Content;
            dynamic f = rng.Find;
            f.ClearFormatting();
            f.Replacement.ClearFormatting();
            // wdFindContinue 1, wdReplaceAll 2.
            f.Execute(FindText: text, MatchCase: false, MatchWholeWord: false, MatchWildcards: false,
                      Forward: true, Wrap: 1, Format: false, ReplaceWith: with, Replace: 2);
            doc.Saved = false;
            return Ok($"replaced {text} with {with}, {count} time(s)");
        }

        private static string WordComment(dynamic doc, string handle, string selector, string text)
        {
            Snapshot(handle);
            var (a, _) = ParaSpan((object)doc, selector, "comment");
            doc.Comments.Add(doc.Paragraphs[a + 1].Range, text ?? "");
            doc.Saved = false;
            return Ok($"comment on p{a}");
        }

        private static string WordLink(dynamic doc, string handle, string selector, string url, string title)
        {
            Snapshot(handle);
            var (a, _) = ParaSpan((object)doc, selector, "link");
            dynamic p = doc.Paragraphs[a + 1];
            // The paragraph without its mark: linking the mark as well drags
            // the next paragraph's formatting into the link.
            dynamic anchor = doc.Range(p.Range.Start, p.Range.End - 1);
            if (string.IsNullOrWhiteSpace(title)) doc.Hyperlinks.Add(Anchor: anchor, Address: url);
            else doc.Hyperlinks.Add(Anchor: anchor, Address: url, TextToDisplay: title);
            doc.Saved = false;
            return Ok($"p{a} links to {url}");
        }

        private static string WordHeader(dynamic doc, string handle, string which, string text)
        {
            Snapshot(handle);
            var footer = (which ?? "").Trim().ToLowerInvariant() switch
            {
                "header" or "" => false,
                "footer" => true,
                _ => throw new InvalidOperationException($"header takes name header or footer, not {which}"),
            };
            int n = doc.Sections.Count;
            for (var i = 1; i <= n; i++)
            {
                dynamic s = doc.Sections[i];
                // wdHeaderFooterPrimary 1: the one on every ordinary page.
                dynamic hf = footer ? s.Footers[1] : s.Headers[1];
                hf.Range.Text = text ?? "";
            }
            doc.Saved = false;
            return Ok($"{(footer ? "footer" : "header")} set on {n} section(s){(footer ? "; it replaces any page number there, so add pageNumbers after it" : "")}");
        }

        private static string WordPageSetup(dynamic doc, string handle, string style)
        {
            Snapshot(handle);
            dynamic ps = doc.PageSetup;
            var did = new List<string>();
            foreach (var (k, v) in Pairs(style))
            {
                switch (k)
                {
                    case "orientation":
                        // wdOrientPortrait 0, wdOrientLandscape 1
                        ps.Orientation = v.StartsWith("land", StringComparison.OrdinalIgnoreCase) ? 1 : 0;
                        break;
                    case "paper":
                        // wdPaperLetter 2, wdPaperLegal 4, wdPaperA3 6, wdPaperA4 7
                        ps.PaperSize = v.ToUpperInvariant() switch
                        {
                            "A4" => 7, "A3" => 6, "LETTER" => 2, "LEGAL" => 4,
                            _ => throw new InvalidOperationException($"paper does not know {v}: A4, A3, Letter or Legal"),
                        };
                        break;
                    case "margin":
                    {
                        var pts = Inches(v) * 72.0;
                        ps.TopMargin = pts; ps.BottomMargin = pts; ps.LeftMargin = pts; ps.RightMargin = pts;
                        break;
                    }
                    default:
                        throw new InvalidOperationException($"pageSetup does not know {k} in Word: orientation, paper, margin");
                }
                did.Add(k);
            }
            if (did.Count == 0) throw new InvalidOperationException("pageSetup was given nothing: try orientation=landscape;margin=1");
            doc.Saved = false;
            return Ok($"page setup: {string.Join(", ", did)}");
        }

        /// Word's built-in styles by their English names. A style set by
        /// name fails in a Word whose interface is in another language --
        /// there "Heading 1" is "Judul 1" or "Überschrift 1" -- so the names
        /// Word ships with go by number (WdBuiltinStyle), which every
        /// language understands. Anything else is still tried by name.
        private static readonly Dictionary<string, int> BuiltinStyles = new(StringComparer.OrdinalIgnoreCase)
        {
            ["Normal"] = -1,
            ["Heading 1"] = -2, ["Heading 2"] = -3, ["Heading 3"] = -4, ["Heading 4"] = -5, ["Heading 5"] = -6,
            ["Heading 6"] = -7, ["Heading 7"] = -8, ["Heading 8"] = -9, ["Heading 9"] = -10,
            ["Caption"] = -35,
            ["List Bullet"] = -49, ["List Number"] = -50,
            ["List Bullet 2"] = -55, ["List Bullet 3"] = -56,
            ["List Number 2"] = -59, ["List Number 3"] = -60,
            ["Title"] = -63, ["Subtitle"] = -75,
            ["List Paragraph"] = -180, ["Quote"] = -181, ["Intense Quote"] = -182,
        };

        // ============================================================ PowerPoint

        private static int SlideNumber(object presObj, string selector, string verb)
        {
            dynamic pres = presObj;
            var m = Regex.Match((selector ?? "").Trim(), @"^s(\d+)$", RegexOptions.IgnoreCase);
            if (!m.Success) throw new InvalidOperationException($"{verb} needs a slide like s3");
            var n = int.Parse(m.Groups[1].Value, CultureInfo.InvariantCulture);
            int count = pres.Slides.Count;
            if (n < 1 || n > count) throw new InvalidOperationException($"slide {n} does not exist: the deck has {count}");
            return n;
        }

        private static string SlideDelete(dynamic pres, string handle, string selector)
        {
            Snapshot(handle);
            var n = SlideNumber((object)pres, selector, "delete");
            pres.Slides[n].Delete();
            return Ok($"slide {n} deleted; the slides after it moved up one, and the deck has {(int)pres.Slides.Count}");
        }

        private static string SlideDuplicate(dynamic pres, string handle, string selector)
        {
            Snapshot(handle);
            var n = SlideNumber((object)pres, selector, "duplicateSlide");
            pres.Slides[n].Duplicate();
            return Ok($"slide {n} duplicated as s{n + 1}; the slides after it moved down one");
        }

        private static string SlideMove(dynamic pres, string handle, string selector, string at)
        {
            Snapshot(handle);
            var n = SlideNumber((object)pres, selector, "moveSlide");
            var to = SlideNumber((object)pres, at, "moveSlide `at`");
            pres.Slides[n].MoveTo(to);
            return Ok($"slide {n} is now s{to}");
        }

        private static string SlideTextBox(dynamic pres, string handle, string selector, string box, string text, string style)
        {
            Snapshot(handle);
            var n = SlideNumber((object)pres, selector, "textBox");
            var (l, t, w, h) = Box(box, 60.0, 140.0, 600.0, 60.0);
            dynamic shape = pres.Slides[n].Shapes.AddTextbox(1, l, t, w, h); // msoTextOrientationHorizontal
            dynamic range = shape.TextFrame.TextRange;
            range.Text = text ?? "";
            foreach (var (k, v) in Pairs(style))
            {
                var on = !(v == "0" || v.Equals("false", StringComparison.OrdinalIgnoreCase));
                switch (k)
                {
                    case "size": range.Font.Size = double.Parse(v, CultureInfo.InvariantCulture); break;
                    case "bold": range.Font.Bold = on ? -1 : 0; break;
                    case "italic": range.Font.Italic = on ? -1 : 0; break;
                    case "font": range.Font.Name = v; break;
                    case "color": range.Font.Color.RGB = OleColor(v); break;
                    case "align":
                        // ppAlignLeft 1, ppAlignCenter 2, ppAlignRight 3
                        range.ParagraphFormat.Alignment = v.ToLowerInvariant() switch
                        {
                            "left" => 1, "center" or "centre" => 2, "right" => 3,
                            _ => throw new InvalidOperationException($"align does not know {v}"),
                        };
                        break;
                    default:
                        throw new InvalidOperationException($"textBox style does not know {k}: size, bold, italic, font, color, align");
                }
            }
            return Ok($"text box on s{n} at {Math.Round(l)},{Math.Round(t)} ({Math.Round(w)}x{Math.Round(h)}), {(text ?? "").Length} chars");
        }

        private static string DeckTheme(dynamic pres, string handle, string path)
        {
            Snapshot(handle);
            var full = Path.GetFullPath(path);
            if (!File.Exists(full)) throw new InvalidOperationException($"no theme or template at {full}");
            if (Path.GetExtension(full).Equals(".thmx", StringComparison.OrdinalIgnoreCase)) pres.ApplyTheme(full);
            else pres.ApplyTemplate(full);
            return Ok($"applied {Path.GetFileName(full)} to every slide");
        }

        private static string DeckFind(dynamic pres, string text)
        {
            CheckFindable(text, "find");
            var hits = new List<string>();
            int n = pres.Slides.Count;
            for (var i = 1; i <= n; i++)
            {
                dynamic slide = pres.Slides[i];
                if (SlideTexts((object)slide).Any(t => t.IndexOf(text, StringComparison.OrdinalIgnoreCase) >= 0)) hits.Add($"s{i}");
                try
                {
                    string notes = slide.NotesPage.Shapes.Placeholders[2].TextFrame.TextRange.Text;
                    if (notes.IndexOf(text, StringComparison.OrdinalIgnoreCase) >= 0) hits.Add($"s{i}.notes");
                }
                catch (COMException) { /* a slide with no notes placeholder */ }
            }
            return Ok(hits.Count == 0 ? $"{text} is on no slide" : $"{text} is on {string.Join(", ", hits)}");
        }

        private static string DeckReplace(dynamic pres, string handle, string text, string with)
        {
            CheckFindable(text, "replace");
            CheckFindable(with, "replace");
            Snapshot(handle);
            var count = 0;
            int n = pres.Slides.Count;
            for (var i = 1; i <= n; i++)
            {
                foreach (var range in SlideTextRanges((object)pres.Slides[i]))
                {
                    // Replace returns the range it changed, or nothing. Start
                    // the next search after it, or replacing "a" with "aa"
                    // would never end.
                    int after = 0;
                    for (var guard = 0; guard < 10000; guard++)
                    {
                        dynamic hit = range.Replace(text, with, after, 0, 0);
                        if (hit == null) break;
                        count++;
                        after = (int)hit.Start + (int)hit.Length - 1;
                    }
                }
            }
            return Ok(count == 0 ? $"{text} is on no slide; nothing changed" : $"replaced {text} with {with}, {count} time(s)");
        }

        /// Every text range on a slide: text boxes, placeholders, table cells.
        private static IEnumerable<dynamic> SlideTextRanges(object slideObj)
        {
            dynamic slide = slideObj;
            int n = slide.Shapes.Count;
            for (var j = 1; j <= n; j++)
            {
                dynamic shape = slide.Shapes[j];
                // HasTextFrame and HasTable are MsoTriState: -1 is true.
                if ((int)shape.HasTextFrame == -1) yield return shape.TextFrame.TextRange;
                if ((int)shape.HasTable == -1)
                {
                    dynamic table = shape.Table;
                    int rows = table.Rows.Count, cols = table.Columns.Count;
                    for (var r = 1; r <= rows; r++)
                        for (var c = 1; c <= cols; c++)
                            yield return table.Cell(r, c).Shape.TextFrame.TextRange;
                }
            }
        }

        private static IEnumerable<string> SlideTexts(object slide)
        {
            foreach (var r in SlideTextRanges(slide))
            {
                string t = r.Text;
                yield return t ?? "";
            }
        }

        // ---- the shared verbs, where an app had gone without ----
        // header, pageNumbers and pageSetup were each missing from one app,
        // so the same request worked in two and was "unsupported" in the
        // third. A model should not have to know which.

        /// Excel's header or footer: the centre section, on one sheet or on
        /// every sheet when no sheet is named.
        private static string ExcelHeader(dynamic wb, string handle, string selector, string which, string text)
        {
            Snapshot(handle);
            var footer = (which ?? "").Trim().ToLowerInvariant() switch
            {
                "header" or "" => false,
                "footer" => true,
                _ => throw new InvalidOperationException($"header takes name header or footer, not {which}"),
            };
            // & starts a code in Excel's header grammar (&P is the page
            // number), so a literal one is doubled.
            var value = (text ?? "").Replace("&", "&&");
            if (value.Length > 250) throw new InvalidOperationException("Excel keeps a header or footer to 255 characters: shorten it");
            var sheets = SheetsNamed((object)wb, selector);
            foreach (var ws in sheets)
            {
                if (footer) ws.PageSetup.CenterFooter = value;
                else ws.PageSetup.CenterHeader = value;
            }
            return Ok($"{(footer ? "footer" : "header")} set on {sheets.Count} sheet(s); it shows when printed or exported to pdf");
        }

        /// Page numbers in the footer of one sheet, or of every sheet.
        private static string ExcelPageNumbers(dynamic wb, string handle, string selector, string text)
        {
            Snapshot(handle);
            var lead = string.IsNullOrWhiteSpace(text) ? "" : text.Replace("&", "&&") + "  ";
            var sheets = SheetsNamed((object)wb, selector);
            foreach (var ws in sheets) ws.PageSetup.CenterFooter = lead + "Page &P of &N";
            return Ok($"page numbers in the footer of {sheets.Count} sheet(s)");
        }

        private static List<dynamic> SheetsNamed(object wbO, string selector)
        {
            dynamic wb = wbO;
            var (sheet, _) = SplitRange(selector ?? "");
            var list = new List<dynamic>();
            if (!string.IsNullOrEmpty(sheet)) { list.Add(Sheet(wb, sheet)); return list; }
            int n = wb.Worksheets.Count;
            for (var i = 1; i <= n; i++) list.Add(wb.Worksheets[i]);
            return list;
        }

        /// A deck's slide size and orientation.
        private static string SlidePageSetup(dynamic pres, string handle, string style)
        {
            Snapshot(handle);
            dynamic ps = pres.PageSetup;
            var did = new List<string>();
            foreach (var (k, v) in Pairs(style))
            {
                switch (k)
                {
                    case "size":
                    case "paper":
                        // Width and height in points for the two screen shapes
                        // everyone means; PowerPoint's own size types for
                        // paper. Either way it rescales what is on the slides.
                        switch (v.Trim().ToLowerInvariant())
                        {
                            case "16:9": case "widescreen": ps.SlideWidth = 960; ps.SlideHeight = 540; break;
                            case "4:3": case "standard": ps.SlideWidth = 720; ps.SlideHeight = 540; break;
                            case "16:10": ps.SlideWidth = 720; ps.SlideHeight = 450; break;
                            case "a4": ps.SlideSize = 3; break;      // ppSlideSizeA4Paper
                            case "letter": ps.SlideSize = 2; break;  // ppSlideSizeLetterPaper
                            default: throw new InvalidOperationException($"size does not know {v}: 16:9, 4:3, 16:10, A4 or Letter");
                        }
                        did.Add($"size {v}");
                        break;
                    case "orientation":
                        // msoOrientationHorizontal 1, msoOrientationVertical 2
                        ps.SlideOrientation = v.StartsWith("port", StringComparison.OrdinalIgnoreCase) ? 2 : 1;
                        did.Add($"orientation {v}");
                        break;
                    default:
                        throw new InvalidOperationException($"pageSetup on a deck takes size (16:9, 4:3, 16:10, A4, Letter) and orientation, not {k}");
                }
            }
            if (did.Count == 0) throw new InvalidOperationException("pageSetup needs style, e.g. size=16:9 or orientation=portrait");
            return Ok($"deck is now {string.Join(", ", did)} ({(double)ps.SlideWidth:0}x{(double)ps.SlideHeight:0} points)");
        }
    }
}
