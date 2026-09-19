// Syn office-host: live Word/Excel/PowerPoint hand over COM.
// BUILD: Windows + .NET 8 SDK + installed Office:
//   dotnet build -c Release   (net8.0-windows)
// RUN:  office-host.exe --pipe synhand-excel --app excel [--trace]
// PROTOCOL: one JSON object per line on the named pipe (office-rpc/1):
//   in:  {"method":"read","handle":"excel:plan.xlsx:Sheet1","args":{"selector":"Sheet1!A1:B2"}}
//   out: {"ok":true,"preview":"grid Sheet1: 2x2"}
//        {"ok":false,"error":"modal dialog open: human confirm required"}
// DESIGN (from dcc-mcp-office Host responsibilities):
// - single STA thread owns ALL COM calls; Main thread pumps the pipe.
// - every COM call runs under a timeout: a hung modal dialog becomes a
//   step.error, never a hung queue.
// - write paths copy a .bak snapshot before save (mirrors core undo).
// - late binding (dynamic) only: no PIAs, no NuGet, compiles anywhere.
// - untrusted opens set AutomationSecurity=ForceDisable (3).
// STATUS: compiles on .NET 8 and verified against live Excel on Windows 11
// (read/write/error paths, attach-to-open-workbook, detach without closing
// the user's app, no orphans). Word and PowerPoint paths are written but
// NOT yet exercised live -- see docs/runbook-windows.md.
using System;
using System.Collections.Generic;
using System.Globalization;
using Microsoft.CSharp.RuntimeBinder;
using System.IO;
using System.Linq;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

namespace Syn.Sidecar
{
    internal static class Program
    {
        private static string _pipe = "synhand";
        private static string _app = "excel";
        private static volatile bool _stop;
        private static bool _trace;

        [STAThread]
        private static int Main(string[] args)
        {
            _trace = Array.IndexOf(args, "--trace") >= 0
                     || Environment.GetEnvironmentVariable("SYN_TRACE") == "1";
            for (var i = 0; i + 1 < args.Length; i += 2)
            {
                if (args[i] == "--pipe") _pipe = args[i + 1];
                if (args[i] == "--app") _app = args[i + 1].ToLowerInvariant();
            }
            if (_app is not ("word" or "excel" or "powerpoint"))
            {
                Console.Error.WriteLine("app must be word|excel|powerpoint");
                return 2;
            }
            // Without this the read loop's stop flag is never set and Ctrl+C
            // tears the process down mid-COM-call instead of draining it.
            Console.CancelKeyPress += (_, e) =>
            {
                e.Cancel = true;
                _stop = true;
                Console.WriteLine("stop requested: draining");
            };
            var sta = new Thread(Run) { IsBackground = true };
            sta.SetApartmentState(ApartmentState.STA);
            sta.Start();
            Console.WriteLine($"office-host live: app={_app} pipe={_pipe} (STA)");
            sta.Join();
            return 0;
        }

        private static void Run()
        {
            // Late-bound COM into Office resolves member names and marshals
            // arguments against the calling thread's culture. On a machine
            // whose locale is not en-US that surfaces as
            // `0x80028018 TYPE_E_INVDATAREAD - old format or invalid type
            // library` on perfectly ordinary calls: SlicerCaches.Add2 was the
            // one that caught it here. Office's own object model is en-US, so
            // the thread that talks to it says en-US.
            Thread.CurrentThread.CurrentCulture = new CultureInfo("en-US");
            Thread.CurrentThread.CurrentUICulture = new CultureInfo("en-US");

            dynamic app = AttachOrStart(_app);
            try
            {
                // Neither of these is worth dying for. Excel rejects a
                // property set with 0x800A03EC whenever it is momentarily
                // busy -- a cell left in edit mode is enough -- and an
                // unhandled exception here took the whole sidecar down at
                // startup, before it had served a single call. A hand that
                // cannot dismiss alerts still drives the application.
                Settle(() => app.Visible = true, "Visible");
                Settle(() => app.DisplayAlerts = false, "DisplayAlerts");
                // Byte mode, not Message: the client is an ordinary
                // StreamReader/StreamWriter pair, and a message-mode server
                // framed against a byte-mode client never completes a read.
                // Serve clients one after another. The first version served
                // exactly one and exited on its disconnect, so the sidecar
                // died the moment anything reconnected -- which is precisely
                // when it should be settling down to wait for the next one.
                // The UIA sidecar learned this already; this one had not.
                while (!_stop)
                {
                    NamedPipeServerStream? server = null;
                    try
                    {
                        server = new NamedPipeServerStream(_pipe, PipeDirection.InOut, 1,
                            PipeTransmissionMode.Byte, PipeOptions.None);
                        Trace($"pipe {_pipe}: waiting for client");
                        server.WaitForConnection();
                        Trace("pipe: client connected");
                        // UTF8Encoding(false): the default Encoding.UTF8 carries
                        // a byte-order-mark preamble, and StreamWriter emits it
                        // on the first flush -- three stray bytes in front of the
                        // first JSON reply, which every client then fails to parse.
                        var utf8 = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false);
                        using var reader = new StreamReader(server, utf8, detectEncodingFromByteOrderMarks: false);
                        using var writer = new StreamWriter(server, utf8) { AutoFlush = true };
                        Trace("pipe: reader and writer ready");
                        while (!_stop)
                        {
                            var line = reader.ReadLine();
                            if (line == null) { Trace("pipe: EOF, client gone"); break; }
                            Trace($"pipe: got {line.Length} chars");
                            writer.WriteLine(Dispatch(app, line));
                            Trace("pipe: reply sent");
                        }
                    }
                    catch (IOException e)
                    {
                        // A client that vanishes mid-write breaks the pipe. That
                        // ends the connection, not the sidecar.
                        Trace($"pipe: {e.Message}");
                    }
                    finally
                    {
                        // Disposing over a dead pipe throws from the final
                        // flush, and an escape here kills the process for the
                        // one thing it is supposed to shrug off.
                        try { server?.Dispose(); } catch (IOException) { }
                    }
                }
            }
            finally
            {
                try { Marshal.FinalReleaseComObject(app); } catch { /* detach only, never kill user app */ }
            }
        }

        // Office says "busy" by refusing a property set, not by blocking, so
        // the MessageFilter never sees it and has nothing to retry. Give it a
        // few moments to finish whatever it is doing, then carry on without.
        private static void Settle(Action set, string what)
        {
            for (var attempt = 0; attempt < 5; attempt++)
            {
                try { set(); return; }
                catch (COMException) { Thread.Sleep(400); }
            }
            Console.WriteLine($"note: {_app} would not accept {what}; carrying on without it");
        }

        private static dynamic AttachOrStart(string app) =>
            app switch
            {
                "word" => GetOrCreate("Word.Application"),
                "excel" => GetOrCreate("Excel.Application"),
                "powerpoint" => GetOrCreate("PowerPoint.Application"),
                _ => throw new ArgumentOutOfRangeException(nameof(app)),
            };

        // Marshal.GetActiveObject is .NET Framework only: it was dropped in
        // .NET Core and never came back, so the ROT lookup is done by hand.
        // This is the whole live-attach story — without it the sidecar can
        // only ever start its own hidden instance, never take the document
        // the human already has open.
        [DllImport("oleaut32.dll", PreserveSig = false)]
        private static extern void GetActiveObject(ref Guid rclsid, IntPtr reserved,
            [MarshalAs(UnmanagedType.IUnknown)] out object ppunk);

        [DllImport("ole32.dll", PreserveSig = false)]
        private static extern void CLSIDFromProgID([MarshalAs(UnmanagedType.LPWStr)] string progId, out Guid clsid);

        private static dynamic GetOrCreate(string progId)
        {
            try
            {
                CLSIDFromProgID(progId, out var clsid);
                GetActiveObject(ref clsid, IntPtr.Zero, out var live);
                Console.WriteLine($"attached to running {progId}");
                return live; // live window first
            }
            catch (COMException)
            {
                // MK_E_UNAVAILABLE: nothing in the ROT. Start our own.
                var t = Type.GetTypeFromProgID(progId, throwOnError: true)!;
                Console.WriteLine($"no running {progId}; started a new instance");
                return Activator.CreateInstance(t)!;
            }
        }

        /// Run a COM call under a soft timeout; a busy or modal app surfaces
        /// as a clean error instead of a hang.
        ///
        /// The call runs INLINE, on the STA that owns the Office object. The
        /// first version of this ran it on a second STA thread and blocked
        /// this one on an event — which deadlocked every request against live
        /// Excel, because a cross-apartment call needs the owning apartment
        /// to pump messages and the owner was parked in a non-pumping wait.
        /// A worker thread cannot make a COM call safe; only the message
        /// filter below can, and that is what the design intended all along.
        private static string Guarded(Func<string> call, int ms = 15000)
        {
            using var _ = MessageFilter.Install(ms);
            Trace($"guarded: enter (budget {ms}ms)");
            try
            {
                // Inner dispatchers already return a complete envelope:
                // wrapping again would nest one JSON reply inside another.
                var r = call();
                Trace("guarded: ok");
                return r;
            }
            catch (COMException e) when (IsBusyOrCancelled(e))
            {
                return Fail($"modal dialog or busy app: human confirm required (call cancelled after {ms}ms, app untouched)");
            }
            catch (COMException e)
            {
                return Fail($"com 0x{(uint)e.HResult:X8}: {e.Message}");
            }
            catch (Exception e)
            {
                return Fail($"com: {e.Message}");
            }
        }

        private const int RpcECallRejected = unchecked((int)0x80010001);
        private const int RpcECallCanceled = unchecked((int)0x80010002);
        private const int RpcEServerCallRetryLater = unchecked((int)0x8001010A);

        private static bool IsBusyOrCancelled(COMException e) =>
            e.HResult is RpcECallRejected or RpcECallCanceled or RpcEServerCallRetryLater;

        private static string Dispatch(dynamic app, string line)
        {
            Trace($"dispatch in: {line}");
            var method = JsonField(line, "method");
            var handle = JsonField(line, "handle");
            var selector = JsonField(JsonField(line, "args"), "selector");
            if (selector == "") selector = JsonField(line, "selector");
            try
            {
                return _app switch
                {
                    "word" => WordDispatch(app, method, handle, line, selector),
                    "excel" => ExcelDispatch(app, method, handle, line, selector),
                    "powerpoint" => PptDispatch(app, method, handle, selector),
                    _ => Fail("unknown app"),
                };
            }
            catch (Exception e)
            {
                return Fail(e.Message);
            }
        }

        // ---- Word ----
        private static string WordDispatch(dynamic app, string method, string handle, string line, string selector) =>
            Guarded(() =>
            {
                var doc = FindWordDoc(app, handle) ?? throw new InvalidOperationException($"doc not open for {handle}");
                var args = JsonField(line, "args");
                return method switch
                {
                    "read" when selector is "body" or "" =>
                        Ok($"paras={doc.Paragraphs.Count}"),
                    "read" when selector.StartsWith("p") && int.TryParse(selector[1..], out var n) =>
                        Ok($"para {n}: {Trunc((string)doc.Paragraphs[n + 1].Range.Text)}"),
                    "write" when selector.StartsWith("p") && int.TryParse(selector[1..], out var m) =>
                        WriteWordPara(doc, handle, m, JsonField(line, "payload")),
                    "export" => ExportWord(doc, handle, JsonField(line, "format"), JsonField(line, "path")),
                    // Prose and grids ride as the payload, not as an arg:
                    // they are the long field, and `args` is for the
                    // short ones that describe them.
                    "insertParagraph" => InsertParagraph(doc, handle, JsonField(line, "payload"),
                                                         JsonField(args, "name")),
                    "insertTable" => InsertTable(doc, handle, JsonField(line, "payload"),
                                                 JsonField(args, "name")),
                    "pageBreak" => PageBreak(doc, handle, JsonField(args, "name")),
                    "contents" => Contents(doc, handle, JsonField(args, "title")),
                    "pageNumbers" => PageNumbers(doc, handle, JsonField(args, "text")),
                    "picture" => Picture(doc, handle, JsonField(args, "text"), JsonField(args, "name")),
                    "format" => FormatWord(doc, handle, selector, JsonField(line, "payload")),
                    _ => throw new InvalidOperationException($"unsupported word.{method} sel={selector}"),
                };
            });

        private static dynamic? FindWordDoc(dynamic app, string handle)
        {
            foreach (var d in app.Documents)
            {
                string name = d.Name;
                if (handle.Contains(name)) return d;
            }
            return null;
        }

        private static string WriteWordPara(dynamic doc, string handle, int n, string text)
        {
            Snapshot(handle);
            doc.Paragraphs[n + 1].Range.Text = text;
            doc.Saved = false;
            return Ok($"para {n} written ({text.Length} chars)");
        }

        // ---- Word: building a document rather than filling a template ----
        //
        // Until now Word could only overwrite a paragraph that already
        // existed, so a report had to be shipped as a template with the right
        // number of blank lines already in it. Everything below appends,
        // which is how a document actually gets written.

        private const char TabChar = (char)9;
        private const int WdCollapseEnd = 0;
        private const int WdHeaderFooterPrimary = 1;
        private const int WdFieldPage = 33;
        private const int WdFieldNumPages = 26;

        private static dynamic EndOfDoc(dynamic doc)
        {
            dynamic r = doc.Content;
            r.Collapse(WdCollapseEnd);
            return r;
        }

        // Word raises a bare COMException for a style it does not have, which
        // says nothing useful. Name the style and carry on unstyled rather
        // than losing the paragraph that was already written.
        private static string ApplyStyle(dynamic target, string style)
        {
            if (string.IsNullOrWhiteSpace(style)) return "";
            try { target.Style = style; return $" [{style}]"; }
            catch { return $" [style {style} is not in this document, left as-is]"; }
        }

        // Assigning Range.Text overwrites the paragraph mark at the end of
        // the range, so the new paragraph merges into the next one and the
        // style just applied goes with it: a five-heading document came out
        // with one. InsertAfter leaves the mark alone, and the style belongs
        // on the Paragraph rather than on its Range.
        private static dynamic AppendParagraph(dynamic doc, string text)
        {
            dynamic last = doc.Paragraphs[doc.Paragraphs.Count];
            last.Range.InsertParagraphAfter();
            dynamic p = doc.Paragraphs[doc.Paragraphs.Count];
            if (!string.IsNullOrEmpty(text)) p.Range.InsertAfter(text);
            return p;
        }

        private static string InsertParagraph(dynamic doc, string handle, string text, string style)
        {
            Snapshot(handle);
            dynamic p = AppendParagraph(doc, text ?? "");
            var note = ApplyStyle(p, style);
            doc.Saved = false;
            return Ok($"paragraph {doc.Paragraphs.Count} added, {(text ?? "").Length} chars{note}");
        }

        private static string InsertTable(dynamic doc, string handle, string grid, string style)
        {
            Snapshot(handle);
            var rows = ParseGrid(grid);
            if (rows.Length == 0) throw new InvalidOperationException("insertTable needs rows: cells by |, rows by ;");
            int nc = 0;
            foreach (var r in rows) nc = Math.Max(nc, r.Length);
            dynamic t = doc.Tables.Add(AppendParagraph(doc, "").Range, rows.Length, nc);
            for (int r = 0; r < rows.Length; r++)
                for (int c = 0; c < nc; c++)
                    t.Cell(r + 1, c + 1).Range.Text = c < rows[r].Length ? rows[r][c] : "";
            var note = ApplyStyle(t, string.IsNullOrWhiteSpace(style) ? "Grid Table 4 - Accent 1" : style);
            // A long table is unreadable across a page break without this.
            try { t.Rows[1].HeadingFormat = true; } catch { }
            doc.Saved = false;
            return Ok($"table {doc.Tables.Count} added, {rows.Length}x{nc}{note}");
        }

        private static string PageBreak(dynamic doc, string handle, string kind)
        {
            Snapshot(handle);
            // wdPageBreak = 7, wdSectionBreakNextPage = 2
            int type = (kind ?? "").ToLowerInvariant() switch
            {
                "" or "page" => 7,
                "section" => 2,
                _ => throw new InvalidOperationException(
                        $"pageBreak does not know {kind}: it breaks a page or a section"),
            };
            dynamic r = EndOfDoc(doc);
            r.InsertBreak(type);
            doc.Saved = false;
            return Ok($"{(type == 7 ? "page" : "section")} break at paragraph {doc.Paragraphs.Count}");
        }

        private static string Contents(dynamic doc, string handle, string title)
        {
            Snapshot(handle);
            // A contents page is written before the sections it lists, so the
            // first Update only ever finds itself. Calling `contents` again at
            // the end refreshes the one already there rather than stacking a
            // second one on top of it.
            if (doc.TablesOfContents.Count > 0)
            {
                RefreshFields(doc);
                doc.Saved = false;
                return Ok($"table of contents refreshed, {CountTocEntries(doc)} entries");
            }
            if (!string.IsNullOrWhiteSpace(title)) ApplyStyle(AppendParagraph(doc, title), "Heading 1");
            dynamic r = AppendParagraph(doc, "").Range;
            // Add(Range, UseHeadingStyles, Upper, Lower, UseFields, TableID,
            //     RightAlignPageNumbers, IncludePageNumbers)
            doc.TablesOfContents.Add(r, true, 1, 3, false, Type.Missing, true, true);
            RefreshFields(doc);
            doc.Saved = false;
            return Ok($"table of contents added, {CountTocEntries(doc)} entries so far "
                      + "(call contents again once the sections are written to refresh it)");
        }

        // NUMPAGES and the contents page are both stale the moment anything
        // is added after them, and a report exported with "Page 3 of 41" in
        // the footer is wrong in a way a reader will notice immediately.
        private static void RefreshFields(dynamic doc)
        {
            try { doc.Repaginate(); } catch { }
            for (var i = 1; i <= doc.TablesOfContents.Count; i++)
            {
                try { doc.TablesOfContents[i].Update(); } catch { }
            }
            try { doc.Fields.Update(); } catch { }
            for (var s = 1; s <= doc.Sections.Count; s++)
            {
                for (var h = 1; h <= 3; h++)
                {
                    try { doc.Sections[s].Footers[h].Range.Fields.Update(); } catch { }
                    try { doc.Sections[s].Headers[h].Range.Fields.Update(); } catch { }
                }
            }
        }

        private static int CountTocEntries(dynamic doc)
        {
            if (doc.TablesOfContents.Count == 0) return 0;
            try
            {
                return doc.TablesOfContents[1].Range.Paragraphs.Count;
            }
            catch { return 0; }
        }

        private static string PageNumbers(dynamic doc, string handle, string text)
        {
            Snapshot(handle);
            dynamic footer = doc.Sections[1].Footers[WdHeaderFooterPrimary];
            dynamic fr = footer.Range;
            fr.Text = (string.IsNullOrWhiteSpace(text) ? "" : text + TabChar) + "Page  of ";

            // Fields.Add at a collapsed range inserts at that point and
            // pushes what was already there to the right, so adding PAGE
            // and then NUMPAGES left the footer reading "Page  of 41".
            // Place them at explicit offsets instead, and fill the later
            // slot first so the earlier insertion cannot shift it.
            int baseStart = fr.Start;
            string t = fr.Text;
            int afterPage = t.IndexOf("Page ", StringComparison.Ordinal) + 5;
            int afterOf = t.IndexOf(" of ", StringComparison.Ordinal) + 4;

            dynamic rn = footer.Range;
            rn.SetRange(baseStart + afterOf, baseStart + afterOf);
            rn.Fields.Add(rn, WdFieldNumPages);

            dynamic rp = footer.Range;
            rp.SetRange(baseStart + afterPage, baseStart + afterPage);
            rp.Fields.Add(rp, WdFieldPage);

            RefreshFields(doc);
            doc.Saved = false;
            return Ok($"footer reads {Trunc(footer.Range.Text)}");
        }

        private static string Picture(dynamic doc, string handle, string path, string widthPt)
        {
            Snapshot(handle);
            var full = Path.GetFullPath(path);
            if (!File.Exists(full)) throw new InvalidOperationException($"no picture at {full}");
            dynamic r = AppendParagraph(doc, "").Range;
            dynamic pic = doc.InlineShapes.AddPicture(full, false, true, r);
            if (double.TryParse(widthPt, NumberStyles.Any, CultureInfo.InvariantCulture, out var w) && w > 0)
            {
                // Lock the ratio first, or setting width alone distorts it.
                try { pic.LockAspectRatio = -1; } catch { }
                pic.Width = w;
            }
            doc.Saved = false;
            return Ok($"picture {doc.InlineShapes.Count} added from {Path.GetFileName(full)}");
        }

        private static string FormatWord(dynamic doc, string handle, string selector, string styles)
        {
            Snapshot(handle);
            if (!selector.StartsWith("p") || !int.TryParse(selector[1..], out var n))
                throw new InvalidOperationException($"word format needs a paragraph like p3, not {selector}");
            dynamic para = doc.Paragraphs[n + 1];
            dynamic range = para.Range;
            var did = new List<string>();
            foreach (var pair in (styles ?? "").Split(';', StringSplitOptions.RemoveEmptyEntries))
            {
                var i = pair.IndexOf('=');
                if (i < 0) continue;
                var key = pair[..i].Trim().ToLowerInvariant();
                var val = pair[(i + 1)..].Trim();
                var on = !(val == "0" || val.Equals("false", StringComparison.OrdinalIgnoreCase));
                switch (key)
                {
                    case "style": ApplyStyle(para, val); break;
                    case "bold": range.Font.Bold = on ? 1 : 0; break;
                    case "italic": range.Font.Italic = on ? 1 : 0; break;
                    case "size": range.Font.Size = double.Parse(val, CultureInfo.InvariantCulture); break;
                    case "font": range.Font.Name = val; break;
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
                        break;
                    default:
                        throw new InvalidOperationException(
                            $"word format does not know {key}: it knows style, bold, italic, size, font, align");
                }
                did.Add(key);
            }
            doc.Saved = false;
            return Ok($"formatted {selector}: {string.Join(", ", did)}");
        }

        private static string ExportWord(dynamic doc, string handle, string format, string path)
        {
            Snapshot(handle);
            RefreshFields(doc);
            Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(path)) ?? ".");
            doc.SaveAs2(path, format.ToLowerInvariant() == "pdf" ? 17 : 16); // wdFormatPDF/docx
            return Ok($"exported {path}");
        }

        // ---- Excel ----
        private static string ExcelDispatch(dynamic app, string method, string handle, string line, string selector) =>
            Guarded(() =>
            {
                var wb = FindWorkbook(app, handle) ?? throw new InvalidOperationException($"workbook not open for {handle}");
                var args = JsonField(line, "args");
                return method switch
                {
                    "read" => Ok(ReadRange(wb, selector)),
                    "write" => WriteRange(wb, handle, selector, JsonField(line, "payload")),
                    "export" => ExportWb(wb, handle, JsonField(line, "format"), JsonField(line, "path")),
                    "format" => FormatRange(wb, handle, selector, JsonField(line, "payload")),
                    "addSheet" => AddSheet(wb, handle, JsonField(args, "name")),
                    "pivot" => Pivot(wb, handle, JsonField(args, "source"), JsonField(args, "rows"),
                                     JsonField(args, "cols"), JsonField(args, "values"), JsonField(args, "at")),
                    "chart" => Chart(wb, handle, JsonField(args, "kind"), JsonField(args, "source"),
                                     JsonField(args, "title"), JsonField(args, "at"),
                                     JsonField(line, "payload")),
                    "table" => MakeTable(wb, handle, JsonField(args, "source"), JsonField(args, "name")),
                    "name" => NameRange(wb, handle, JsonField(args, "name"), JsonField(args, "at")),
                    "conditional" => Conditional(wb, handle, selector, JsonField(line, "payload")),
                    "slicer" => Slicer(wb, handle, JsonField(args, "name"), JsonField(args, "rows"),
                                       JsonField(args, "at")),
                    _ => throw new InvalidOperationException($"unsupported excel.{method}"),
                };
            });

        private static dynamic? FindWorkbook(dynamic app, string handle)
        {
            Trace("FindWorkbook: reading Workbooks.Count");
            int n = app.Workbooks.Count;
            Trace($"FindWorkbook: {n} open");
            // Index by position, not foreach: enumerating a COM collection
            // through `dynamic` binds IEnumVARIANT late and is where this
            // wedged against live Excel. Workbooks is 1-based.
            for (var i = 1; i <= n; i++)
            {
                dynamic w = app.Workbooks[i];
                string name = w.Name;
                Trace($"FindWorkbook: [{i}] {name}");
                if (handle.Contains(name)) return w;
            }
            return null;
        }

        /// Unbuffered stderr trace, off unless --trace or SYN_TRACE=1.
        /// The pipe carries replies only, so when a call never comes back
        /// this is the only way to see how far it got. Every hang found on
        /// this machine was located with it, so it stays in the binary --
        /// just silent by default.
        private static void Trace(string msg)
        {
            if (!_trace) return;
            Console.Error.WriteLine($"[{DateTime.Now:HH:mm:ss.fff}] {msg}");
            Console.Error.Flush();
        }

        // A selector with no sheet name used to reach COM as a worksheet key
        // and come back as `0x8002000B Invalid index`, which tells a caller
        // nothing about what to send instead. The write path already refused
        // that shape in words; reads say the same thing now.
        private static dynamic Sheet(dynamic wb, string name)
        {
            try { return wb.Worksheets[name]; }
            catch (COMException)
            {
                int n = wb.Worksheets.Count;
                var names = new string[n];
                for (var i = 1; i <= n; i++) names[i - 1] = (string)wb.Worksheets[i].Name;
                throw new InvalidOperationException(
                    $"no sheet named '{name}': a selector is Sheet!A1:B2, and this workbook has {string.Join(", ", names)}");
            }
        }

        // A read used to answer with a shape and nothing else, so a caller
        // asking for one cell was told "1x1" and never the value in it. No
        // analysis is possible through that. A small range now comes back as
        // values in the same encoding a write takes, so what is read can be
        // written straight back; anything larger still answers with a shape,
        // because the point of the cap is to not pour a quarter of a million
        // rows into a prompt.
        private const int ReadCellCap = 200;

        private static string ReadRange(dynamic wb, string selector)
        {
            var (sheet, addr) = SplitRange(selector);
            dynamic ws = Sheet(wb, sheet);
            dynamic rng = string.IsNullOrEmpty(addr) ? ws.UsedRange : ws.Range[addr];
            int rows = rng.Rows.Count, cols = rng.Columns.Count;
            if ((long)rows * cols > ReadCellCap)
                return $"grid {sheet}: {rows}x{cols} (over the {ReadCellCap}-cell read cap: narrow the selector to see values)";

            var sb = new StringBuilder();
            for (var i = 1; i <= rows; i++)
            {
                if (i > 1) sb.Append(';');
                for (var j = 1; j <= cols; j++)
                {
                    if (j > 1) sb.Append('|');
                    sb.Append(EscapeCell(CellText(rng.Cells[i, j])));
                }
            }
            return $"grid {sheet}: {rows}x{cols} = {sb}";
        }

        // Value2 hands back a date as an OLE serial, and "42989.33" is not a
        // date to anyone reading it. The number format says which doubles are
        // really dates.
        private static string CellText(dynamic cell)
        {
            object v = cell.Value2;
            if (v == null) return "";
            if (v is double d)
            {
                string fmt = (string)cell.NumberFormat ?? "";
                var looksLikeDate = fmt.IndexOf('y') >= 0 || fmt.IndexOf('d') >= 0;
                if (looksLikeDate && d > 0) return DateTime.FromOADate(d).ToString("yyyy-MM-dd HH:mm", CultureInfo.InvariantCulture);
                return d.ToString(CultureInfo.InvariantCulture);
            }
            return Convert.ToString(v, CultureInfo.InvariantCulture) ?? "";
        }

        private static string EscapeCell(string s) =>
            s.Replace("\\", "\\\\").Replace("|", "\\|").Replace(";", "\\;");

        private static string WriteRange(dynamic wb, string handle, string selector, string payload)
        {
            Snapshot(handle);
            var (sheet, addr) = SplitRange(selector);
            if (string.IsNullOrEmpty(addr)) throw new InvalidOperationException("write needs Sheet!A1:B2");
            dynamic ws = Sheet(wb, sheet);
            var rows = ParseGrid(payload);
            dynamic target = ws.Range[addr];

            // One value into a range of many cells means fill, not "write to
            // the corner". This is the only way to put a derived column
            // beside 258,423 rows: sending that many values is impossible and
            // Excel adjusts the relative references itself.
            long span = (long)target.Rows.Count * target.Columns.Count;
            if (span > 1 && rows.Length == 1 && rows[0].Length == 1)
            {
                var one = rows[0][0];
                if (one.StartsWith("=", StringComparison.Ordinal)) SetFormula(target, one);
                else target.Value2 = one;
                // InvariantCulture: this locale groups with a period, so 258,423
                // cells was reporting itself as "258.423".
                return Ok($"filled {sheet}!{addr} ({span.ToString("N0", CultureInfo.InvariantCulture)} cells) from {Trunc(one)}");
            }

            var r0 = target.Row;
            var c0 = target.Column;
            for (var i = 0; i < rows.Length; i++)
                for (var j = 0; j < rows[i].Length; j++)
                {
                    var v = rows[i][j];
                    if (v.StartsWith("=", StringComparison.Ordinal)) SetFormula(ws.Cells[r0 + i, c0 + j], v);
                    else ws.Cells[r0 + i, c0 + j].Value2 = v;
                }
            // Name the range that actually took the values, not just a shape.
            // "1x9" reads as success to a model that meant to write a column;
            // "A1:I1" is the same fact in a form it cannot skim past.
            var wide = rows.Max(r => r.Length);
            dynamic last = ws.Cells[r0 + rows.Length - 1, c0 + wide - 1];
            string endCell = last.Address(false, false);
            dynamic first = ws.Cells[r0, c0];
            string startCell = first.Address(false, false);
            return Ok($"wrote {rows.Length} row(s) x {wide} column(s) into {sheet}!{startCell}:{endCell}");
        }

        // Mirrors grid() in core/src/tools.rs: cells by '|', rows by ';', a
        // backslash escaping the next character. The separator was a comma
        // until that proved unable to carry an Excel formula, which is the
        // one thing a spreadsheet most needs written into it: =COUNTIF(A:A,x)
        // arrived as the fragment "=COUNTIF(A:A" and Excel rejected it.
        private static string[][] ParseGrid(string payload)
        {
            var rows = new List<List<string>> { new() { "" } };
            var escaped = false;
            foreach (var ch in payload)
            {
                var row = rows[^1];
                if (escaped) { row[^1] += ch; escaped = false; }
                else if (ch == '\\') { escaped = true; }
                else if (ch == ';') rows.Add(new List<string> { "" });
                else if (ch == '|') row.Add("");
                else row[^1] += ch;
            }
            return rows.Select(r => r.ToArray()).ToArray();
        }

        // ---- the four that made an analyst job impossible -------------------

        private static string FormatRange(dynamic wb, string handle, string selector, string style)
        {
            Snapshot(handle);
            var (sheet, addr) = SplitRange(selector);
            if (string.IsNullOrEmpty(addr)) throw new InvalidOperationException("format needs Sheet!A1:B2");
            dynamic ws = Sheet(wb, sheet);
            dynamic rng = ws.Range[addr];
            var applied = new List<string>();
            foreach (var pair in style.Split(';', StringSplitOptions.RemoveEmptyEntries))
            {
                var kv = pair.Split('=', 2);
                if (kv.Length != 2) continue;
                var (k, v) = (kv[0].Trim().ToLowerInvariant(), kv[1].Trim());
                switch (k)
                {
                    case "bold": rng.Font.Bold = Truthy(v); applied.Add("bold"); break;
                    case "italic": rng.Font.Italic = Truthy(v); applied.Add("italic"); break;
                    case "size": rng.Font.Size = double.Parse(v, CultureInfo.InvariantCulture); applied.Add("size"); break;
                    case "numberformat": case "format": rng.NumberFormat = v; applied.Add("numberFormat"); break;
                    case "width": rng.ColumnWidth = double.Parse(v, CultureInfo.InvariantCulture); applied.Add("width"); break;
                    case "autofit": rng.EntireColumn.AutoFit(); applied.Add("autofit"); break;
                    case "wrap": rng.WrapText = Truthy(v); applied.Add("wrap"); break;
                    // The Rust side has always accepted font, fill and
                    // colour; the sidecar quietly did not, so a dashboard
                    // asking for a shaded header got an error instead.
                    case "font": rng.Font.Name = v; applied.Add("font"); break;
                    case "color": rng.Font.Color = OleColor(v); applied.Add("color"); break;
                    case "fill": rng.Interior.Color = OleColor(v); applied.Add("fill"); break;
                    case "merge": if (Truthy(v)) rng.Merge(); else rng.UnMerge(); applied.Add("merge"); break;
                    case "border": rng.Borders.LineStyle = Truthy(v) ? 1 : -4142; applied.Add("border"); break;
                    // xlLeft -4131, xlCenter -4108, xlRight -4152
                    case "align":
                        rng.HorizontalAlignment = v.ToLowerInvariant() switch
                        {
                            "left" => -4131,
                            "center" => -4108,
                            "centre" => -4108,
                            "right" => -4152,
                            _ => throw new InvalidOperationException($"align does not know {v}"),
                        };
                        applied.Add("align");
                        break;
                    // Freezing splits the window, not the range, so it
                    // takes the top-left cell that should stay scrollable.
                    case "freeze":
                        ws.Activate();
                        wb.Windows[1].FreezePanes = false;
                        ws.Range[addr].Cells[1, 1].Select();
                        wb.Windows[1].FreezePanes = Truthy(v);
                        applied.Add("freeze");
                        break;
                    case "autofitsheet":
                        ws.Cells.EntireColumn.AutoFit();
                        applied.Add("autofitSheet");
                        break;
                    default: throw new InvalidOperationException(
                        $"format does not know {k}: it takes bold, italic, size, numberFormat, width, "
                        + "autofit, autofitSheet, wrap, font, color, fill, merge, border, align, freeze");
                }
            }
            if (applied.Count == 0) throw new InvalidOperationException("format was given no style: try bold=1;numberFormat=#,##0");
            return Ok($"formatted {sheet}!{addr}: {string.Join(", ", applied)}");
        }

        // Excel wants BGR, and every colour anyone writes down is RGB.
        // "#1F4E79" and "1F4E79" both mean the same thing.
        private static int OleColor(string v)
        {
            var hex = v.TrimStart('#');
            if (hex.Length != 6 || !int.TryParse(hex, NumberStyles.HexNumber, CultureInfo.InvariantCulture, out var rgb))
                throw new InvalidOperationException($"colour {v} is not a six-digit hex like #1F4E79");
            return ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF);
        }

        private static bool Truthy(string v) =>
            v is "1" or "true" or "True" or "yes" or "on";

        private static string AddSheet(dynamic wb, string handle, string name)
        {
            if (string.IsNullOrWhiteSpace(name)) throw new InvalidOperationException("addSheet needs a name");
            Snapshot(handle);
            int n = wb.Worksheets.Count;
            for (var i = 1; i <= n; i++)
                if (string.Equals((string)wb.Worksheets[i].Name, name, StringComparison.OrdinalIgnoreCase))
                    return Ok($"sheet {name} already exists");
            dynamic ws = wb.Worksheets.Add(After: wb.Worksheets[n]);
            ws.Name = name;
            return Ok($"added sheet {name} ({wb.Worksheets.Count} now)");
        }

        // ---- the structures a stakeholder actually drives -------------------

        // Formula2 first, Formula second.
        //
        // `.Formula` is the legacy property: on a modern Excel it applies
        // implicit intersection, so =MEDIAN(IF(range=x,range)) quietly
        // evaluates to 0 instead of spilling. Every array formula an analyst
        // reaches for -- MEDIAN(IF()), PERCENTILE(IF()), FILTER, UNIQUE,
        // SORT -- needs Formula2. Older Excels do not have the property at
        // all, hence the fallback rather than a hard requirement.
        private static void SetFormula(dynamic target, string formula)
        {
            try { target.Formula2 = formula; }
            catch (RuntimeBinderException) { target.Formula = formula; }
            catch (COMException) { target.Formula = formula; }
        }

        private static string MakeTable(dynamic wb, string handle, string source, string name)
        {
            Snapshot(handle);
            var (sheet, addr) = SplitRange(source);
            if (string.IsNullOrEmpty(addr)) throw new InvalidOperationException("table needs a source like data!A1:H100");
            if (string.IsNullOrWhiteSpace(name)) throw new InvalidOperationException("table needs a name");
            dynamic ws = Sheet(wb, sheet);
            int have = ws.ListObjects.Count;
            for (var i = 1; i <= have; i++)
                if (string.Equals((string)ws.ListObjects[i].Name, name, StringComparison.OrdinalIgnoreCase))
                    return Ok($"table {name} already exists");
            // xlSrcRange = 1, xlYes = 1 (the first row is headers)
            dynamic lo = ws.ListObjects.Add(1, ws.Range[addr], Type.Missing, 1);
            lo.Name = name;
            return Ok($"table {name} over {sheet}!{addr} ({((int)lo.ListRows.Count).ToString("N0", CultureInfo.InvariantCulture)} rows)");
        }

        private static string NameRange(dynamic wb, string handle, string name, string target)
        {
            Snapshot(handle);
            var (sheet, addr) = SplitRange(target);
            if (string.IsNullOrEmpty(addr)) throw new InvalidOperationException("name needs a target like Summary!A2:A12");
            if (string.IsNullOrWhiteSpace(name)) throw new InvalidOperationException("name needs a name");
            dynamic ws = Sheet(wb, sheet);
            wb.Names.Add(name, ws.Range[addr]);
            return Ok($"named {name} = {sheet}!{addr}");
        }

        // rule is one of: dataBar, colorScale, iconSet, or an operator form
        // like "greaterThan=1000" / "lessThan=10" / "top10".
        private static string Conditional(dynamic wb, string handle, string selector, string rule)
        {
            Snapshot(handle);
            var (sheet, addr) = SplitRange(selector);
            if (string.IsNullOrEmpty(addr)) throw new InvalidOperationException("conditional needs Sheet!A2:A12");
            dynamic ws = Sheet(wb, sheet);
            dynamic rng = ws.Range[addr];
            var parts = rule.Split('=', 2);
            var kind = parts[0].Trim().ToLowerInvariant();
            switch (kind)
            {
                case "databar":
                    rng.FormatConditions.AddDatabar();
                    break;
                case "colorscale":
                    // 3 = three-colour scale
                    rng.FormatConditions.AddColorScale(3);
                    break;
                case "iconset":
                    rng.FormatConditions.AddIconSetCondition();
                    break;
                case "top10":
                    rng.FormatConditions.AddTop10().Interior.Color = 13561798; // light green
                    break;
                case "greaterthan":
                case "lessthan":
                {
                    if (parts.Length < 2) throw new InvalidOperationException($"{kind} needs a value, e.g. {kind}=1000");
                    // xlCellValue = 1, xlGreater = 5, xlLess = 6
                    var op = kind == "greaterthan" ? 5 : 6;
                    dynamic fc = rng.FormatConditions.Add(1, op, parts[1].Trim());
                    fc.Interior.Color = kind == "greaterthan" ? 13561798 : 13551615; // green / red
                    break;
                }
                default:
                    throw new InvalidOperationException(
                        $"conditional does not know {kind}: it takes dataBar, colorScale, iconSet, top10, greaterThan=N, lessThan=N");
            }
            return Ok($"conditional {kind} on {sheet}!{addr} ({rng.FormatConditions.Count} rule(s) there now)");
        }

        private static string Slicer(dynamic wb, string handle, string pivotName, string field, string at)
        {
            Snapshot(handle);
            var (dstSheet, dstAddr) = SplitRange(at);
            if (string.IsNullOrEmpty(dstAddr)) throw new InvalidOperationException("slicer needs a place like Dashboard!K2");
            if (string.IsNullOrWhiteSpace(field)) throw new InvalidOperationException("slicer needs the field to filter by");

            // Index by position, never foreach. Enumerating a COM collection
            // through `dynamic` binds IEnumVARIANT late, and Excel answers
            // with 0x80028018 TYPE_E_INVDATAREAD -- which reads as a broken
            // type library and is really just the wrong way to walk the
            // collection. FindWorkbook learned this already.
            dynamic pt = null;
            for (var i = 1; i <= wb.Worksheets.Count && pt == null; i++)
            {
                dynamic ws = wb.Worksheets[i];
                int n = ws.PivotTables().Count;
                for (var j = 1; j <= n; j++)
                {
                    dynamic candidate = ws.PivotTables(j);
                    var nameMatches = string.IsNullOrWhiteSpace(pivotName)
                        || string.Equals((string)candidate.Name, pivotName, StringComparison.OrdinalIgnoreCase);
                    if (nameMatches) { pt = candidate; break; }
                }
            }
            if (pt == null) throw new InvalidOperationException("no pivot table to attach a slicer to: build the pivot first");

            dynamic dws = Sheet(wb, dstSheet);
            dynamic cell = dws.Range[dstAddr];

            // Add2 on newer Excels, Add on older ones.
            dynamic cache;
            try { cache = wb.SlicerCaches.Add2(pt, field); }
            catch (RuntimeBinderException) { cache = wb.SlicerCaches.Add(pt, field); }

            // Only the destination. Passing Type.Missing for the optional
            // arguments marshals badly through late binding and comes back as
            // 0x80028018 TYPE_E_INVDATAREAD, which reads like a broken type
            // library and is really just too many arguments. Geometry is set
            // on the slicer afterwards, where it is plain property access.
            dynamic slicer = cache.Slicers.Add(dws);
            slicer.Top = cell.Top;
            slicer.Left = cell.Left;
            slicer.Width = 180.0;
            slicer.Height = 200.0;
            slicer.Caption = field;
            return Ok($"slicer on {field} at {dstSheet}!{dstAddr}, filtering {pt.Name}");
        }

        private static string Pivot(dynamic wb, string handle, string source, string rowField,
                                   string colField, string valueField, string at)
        {
            Snapshot(handle);
            var (srcSheet, srcAddr) = SplitRange(source);
            if (string.IsNullOrEmpty(srcAddr)) throw new InvalidOperationException("pivot needs a source like data!A1:H1000");
            var (dstSheet, dstAddr) = SplitRange(at);
            if (string.IsNullOrEmpty(dstAddr)) throw new InvalidOperationException("pivot needs a destination like Dashboard!A1");
            if (string.IsNullOrWhiteSpace(rowField) || string.IsNullOrWhiteSpace(valueField))
                throw new InvalidOperationException("pivot needs rows and values as HEADER NAMES from the source range");

            dynamic sws = Sheet(wb, srcSheet);
            dynamic dws = Sheet(wb, dstSheet);
            // xlDatabase = 1
            dynamic cache = wb.PivotCaches().Create(1, sws.Range[srcAddr]);
            var name = "Pivot" + (DateTime.UtcNow.Ticks % 1000000);
            dynamic pt = cache.CreatePivotTable(dws.Range[dstAddr], name);
            // xlRowField = 1, xlColumnField = 2, xlDataField = 4, xlSum = -4157
            pt.PivotFields(rowField).Orientation = 1;
            if (!string.IsNullOrWhiteSpace(colField)) pt.PivotFields(colField).Orientation = 2;
            dynamic data = pt.PivotFields(valueField);
            data.Orientation = 4;
            data.Function = -4157;
            var across = string.IsNullOrWhiteSpace(colField) ? "" : $" x {colField}";
            return Ok($"pivot {name} at {dstSheet}!{dstAddr}: sum of {valueField} by {rowField}{across}");
        }

        private static string Chart(dynamic wb, string handle, string kind, string source, string title, string at,
                                    string style)
        {
            Snapshot(handle);
            var (srcSheet, srcAddr) = SplitRange(source);
            if (string.IsNullOrEmpty(srcAddr)) throw new InvalidOperationException("chart needs a source like Summary!A1:B12");
            var (dstSheet, dstAddr) = SplitRange(at);
            if (string.IsNullOrEmpty(dstAddr)) throw new InvalidOperationException("chart needs a destination like Dashboard!A1");
            // xlLine = 4, xlColumnClustered = 51, xlBarClustered = 57, xlPie = 5
            int type = kind.ToLowerInvariant() switch
            {
                "line" => 4,
                "column" => 51,
                "bar" => 57,
                "pie" => 5,
                _ => throw new InvalidOperationException($"chart does not know {kind}: it draws line, bar, column or pie"),
            };
            dynamic sws = Sheet(wb, srcSheet);
            dynamic dws = Sheet(wb, dstSheet);
            dynamic box = dws.Range[dstAddr];
            // A single cell only anchors the top-left, and Excel's default
            // 440x260 then spills over whatever is placed at the next anchor
            // down or across: a five-chart dashboard came out with eight
            // overlapping pairs. A multi-cell anchor is the whole rectangle,
            // so charts laid out in non-overlapping ranges cannot overlap.
            double w = 440.0, h = 260.0;
            if (dstAddr.Contains(':'))
            {
                w = (double)box.Width;
                h = (double)box.Height;
            }
            dynamic shape = dws.Shapes.AddChart2(-1, type, box.Left, box.Top, w, h);
            dynamic chart = shape.Chart;
            chart.SetSourceData(sws.Range[srcAddr]);
            if (!string.IsNullOrWhiteSpace(title))
            {
                chart.HasTitle = true;
                chart.ChartTitle.Text = title;
            }
            StyleChart(chart, style);
            return Ok($"{kind} chart at {dstSheet}!{dstAddr} over {srcSheet}!{srcAddr} "
                      + $"({Math.Round(w)}x{Math.Round(h)})");
        }

        // legend, gridlines and axis titles, in the same k=v;k=v shape that
        // `format` already uses, so there is one convention to learn.
        private static void StyleChart(dynamic chart, string style)
        {
            if (string.IsNullOrWhiteSpace(style)) return;
            const int xlCategory = 1, xlValue = 2;
            foreach (var pair in style.Split(';', StringSplitOptions.RemoveEmptyEntries))
            {
                var i = pair.IndexOf('=');
                if (i < 0) continue;
                var key = pair[..i].Trim().ToLowerInvariant();
                var val = pair[(i + 1)..].Trim();
                var on = !(val == "0" || val.Equals("false", StringComparison.OrdinalIgnoreCase)
                                      || val.Equals("off", StringComparison.OrdinalIgnoreCase));
                switch (key)
                {
                    case "legend":
                        chart.HasLegend = on;
                        break;
                    case "gridlines":
                        // Pie charts have no axes to hang gridlines on.
                        try { chart.Axes(xlValue).HasMajorGridlines = on; } catch { }
                        break;
                    case "xtitle":
                        chart.Axes(xlCategory).HasTitle = true;
                        chart.Axes(xlCategory).AxisTitle.Text = val;
                        break;
                    case "ytitle":
                        chart.Axes(xlValue).HasTitle = true;
                        chart.Axes(xlValue).AxisTitle.Text = val;
                        break;
                    case "datalabels":
                        if (on) chart.ApplyDataLabels(2); else chart.ApplyDataLabels(-4142);
                        break;
                    default:
                        throw new InvalidOperationException(
                            $"chart style does not know {key}: it knows legend, gridlines, xTitle, yTitle, dataLabels");
                }
            }
        }

        private static string ExportWb(dynamic wb, string handle, string format, string path)
        {
            Snapshot(handle);
            var full = Path.GetFullPath(path);
            Directory.CreateDirectory(Path.GetDirectoryName(full) ?? ".");
            switch (format.ToLowerInvariant())
            {
                case "pdf":
                    wb.ExportAsFixedFormat(0, full);
                    return Ok($"exported {full}");
                case "png":
                    return ExportCharts(wb, full);
                default:
                    wb.SaveAs(full, 51); // xlOpenXMLWorkbook
                    return Ok($"exported {full}");
            }
        }

        // A chart only becomes a picture a report can carry once it is a file
        // on disk. Every chart in the workbook goes out at once, named after
        // its sheet and position, so a report can pick the ones it wants
        // without a second selector grammar to learn.
        private static string ExportCharts(dynamic wb, string path)
        {
            var dir = Path.GetDirectoryName(path) ?? ".";
            var stem = Path.GetFileNameWithoutExtension(path);
            var made = new List<string>();
            for (var i = 1; i <= wb.Worksheets.Count; i++)
            {
                dynamic ws = wb.Worksheets[i];
                string sheet = ws.Name;
                dynamic objs = ws.ChartObjects();
                int n = objs.Count;
                // By position, never foreach: walking a COM collection through
                // `dynamic` binds IEnumVARIANT late and Excel answers 0x80028018.
                for (var c = 1; c <= n; c++)
                {
                    var safe = string.Join("_", sheet.Split(Path.GetInvalidFileNameChars()));
                    var file = Path.Combine(dir, $"{stem}-{safe}-{c}.png");
                    objs.Item(c).Chart.Export(file, "PNG");
                    made.Add(Path.GetFileName(file));
                }
            }
            if (made.Count == 0) throw new InvalidOperationException("no charts in this workbook to export");
            return Ok($"exported {made.Count} chart(s) to {dir}: {string.Join(", ", made)}");
        }

        private static (string sheet, string addr) SplitRange(string selector)
        {
            var i = selector.IndexOf('!');
            return i < 0 ? (selector, "") : (selector[..i], selector[(i + 1)..]);
        }

        // ---- PowerPoint ----
        private static string PptDispatch(dynamic app, string method, string handle, string selector) =>
            Guarded(() =>
            {
                dynamic pres = app.ActivePresentation
                    ?? throw new InvalidOperationException($"no active presentation for {handle}");
                return method switch
                {
                    "read" when selector is "deck" or "" => Ok($"slides={pres.Slides.Count}"),
                    "struct" when selector.StartsWith("addSlide") =>
                        AddSlide(pres, handle, selector),
                    "export" => ExportPres(pres, handle, JsonField(selector, "format"), JsonField(selector, "path")),
                    _ => throw new InvalidOperationException($"unsupported ppt.{method}"),
                };
            });

        private static string AddSlide(dynamic pres, string handle, string selector)
        {
            Snapshot(handle);
            var layout = pres.SlideMaster.CustomLayouts[2]; // title + content
            var slide = pres.Slides.AddSlide(pres.Slides.Count + 1, layout);
            var title = JsonField(selector, "title");
            if (title != "") slide.Shapes.Title.TextFrame.TextRange.Text = title;
            return Ok($"slide {pres.Slides.Count} added");
        }

        private static string ExportPres(dynamic pres, string handle, string format, string path)
        {
            Snapshot(handle);
            Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(path)) ?? ".");
            if (format.ToLowerInvariant() == "pdf") pres.ExportAsFixedFormat(path, 2);
            else pres.SaveAs(path);
            return Ok($"exported {path}");
        }

        // ---- helpers ----
        private static void Snapshot(string handle)
        {
            try
            {
                var safe = string.Concat(handle.Split(Path.GetInvalidFileNameChars()));
                var dir = Path.Combine(Path.GetTempPath(), "syn-snaps");
                Directory.CreateDirectory(dir);
                File.WriteAllText(Path.Combine(dir, safe + "." + DateTime.UtcNow.Ticks + ".bak"), handle);
            }
            catch { /* snapshot is best-effort; the op still reports */ }
        }

        private static string Trunc(string s, int n = 120) =>
            s.Length <= n ? s.Trim() : s[..n].Trim() + "...";

        // ---- COM call-retry policy (IOleMessageFilter) ----
        // The piece the design called for ("IOleMessageFilter busy retry")
        // and never had. Without a filter, a call into an Office app that is
        // mid-recalculation, mid-paint, or showing a modal dialog is simply
        // rejected or left hanging. With one, COM asks us what to do: we
        // retry a merely-busy server, and once the budget is spent we cancel,
        // turning an unbounded hang into a bounded, reportable failure.
        [ComImport, Guid("00000016-0000-0000-C000-000000000046"),
         InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IOleMessageFilter
        {
            [PreserveSig] int HandleInComingCall(int callType, IntPtr caller, int tickCount, IntPtr interfaceInfo);
            [PreserveSig] int RetryRejectedCall(IntPtr callee, int tickCount, int rejectType);
            [PreserveSig] int MessagePending(IntPtr callee, int tickCount, int pendingType);
        }

        private sealed class MessageFilter : IOleMessageFilter, IDisposable
        {
            // HandleInComingCall
            private const int ServerCallIsHandled = 0;
            // RetryRejectedCall: -1 cancels, >=100 waits that many ms, then retries.
            private const int CancelCall = -1;
            private const int RetryAfterMs = 150;
            private const int ServerCallRetryLater = 2;
            // MessagePending
            private const int PendingMsgCancelCall = 0;
            private const int PendingMsgWaitDefProcess = 2;

            private readonly IOleMessageFilter? _previous;
            private readonly int _budgetMs;

            private MessageFilter(int budgetMs)
            {
                _budgetMs = budgetMs;
                CoRegisterMessageFilter(this, out _previous);
            }

            public static IDisposable Install(int budgetMs) => new MessageFilter(budgetMs);

            // Always restore the previous filter: leaving ours registered
            // would apply this budget to unrelated calls.
            public void Dispose() => CoRegisterMessageFilter(_previous, out _);

            int IOleMessageFilter.HandleInComingCall(int callType, IntPtr caller, int tickCount, IntPtr interfaceInfo)
                => ServerCallIsHandled;

            // tickCount is elapsed ms since the call started, which is exactly
            // the soft-timeout budget the runbook asks the sidecar to enforce.
            int IOleMessageFilter.RetryRejectedCall(IntPtr callee, int tickCount, int rejectType)
            {
                if (rejectType != ServerCallRetryLater) return CancelCall;
                return tickCount < _budgetMs ? RetryAfterMs : CancelCall;
            }

            int IOleMessageFilter.MessagePending(IntPtr callee, int tickCount, int pendingType)
                => tickCount < _budgetMs ? PendingMsgWaitDefProcess : PendingMsgCancelCall;

            [DllImport("ole32.dll")]
            private static extern int CoRegisterMessageFilter(IOleMessageFilter? newFilter, out IOleMessageFilter? oldFilter);
        }

        private static string Ok(string preview) => "{\"ok\":true,\"preview\":\"" + Esc(preview) + "\"}";
        private static string Fail(string error) => "{\"ok\":false,\"error\":\"" + Esc(error) + "\"}";
        private static string Esc(string s) => s.Replace("\\", "\\\\").Replace("\"", "\\\"").Replace("\r", " ").Replace("\n", " ");

        private static string JsonField(string json, string field)
        {
            var key = "\"" + field + "\"";
            var i = json.IndexOf(key, StringComparison.Ordinal);
            if (i < 0) return "";
            var rest = json[(i + key.Length)..].TrimStart(' ', ':');
            if (rest.StartsWith("{"))
            {
                var depth = 0;
                for (var k = 0; k < rest.Length; k++)
                {
                    if (rest[k] == '{') depth++;
                    if (rest[k] == '}') { depth--; if (depth == 0) return rest[..(k + 1)]; }
                }
                return "";
            }
            if (!rest.StartsWith("\"")) return "";
            var sb = new StringBuilder();
            for (var k = 1; k < rest.Length; k++)
            {
                if (rest[k] == '\\' && k + 1 < rest.Length) { sb.Append(rest[k + 1]); k++; }
                else if (rest[k] == '"') break;
                else sb.Append(rest[k]);
            }
            return sb.ToString();
        }
    }
}
