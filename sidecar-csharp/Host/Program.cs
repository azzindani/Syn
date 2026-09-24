// Syn office-host: live Word/Excel/PowerPoint hand over COM.
// BUILD: Windows + .NET 8 SDK + installed Office:
//   dotnet build -c Release   (net8.0-windows)
// RUN:  office-host.exe --pipe hand-excel --app excel [--trace]
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
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Globalization;
using Microsoft.CSharp.RuntimeBinder;
using System.IO;
using System.Linq;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace Syn.Sidecar
{
    internal static class Program
    {
        private static string _pipe = "hand";
        private static string _app = "excel";
        private static volatile bool _stop;
        private static bool _trace;

        /// <summary>How many clients may be connected at once: the console,
        /// a few MCP clients, a terminal.</summary>
        private const int MaxClients = 8;

        /// <summary>One request line and the reply the STA thread gives it.</summary>
        private sealed record Job(string Line)
        {
            public TaskCompletionSource<string> Reply { get; } =
                new(TaskCreationOptions.RunContinuationsAsynchronously);
        }

        /// <summary>Requests from every connected client, in arrival order,
        /// for the one thread that owns COM.</summary>
        private static readonly BlockingCollection<Job> Jobs = new();

        [STAThread]
        private static int Main(string[] args)
        {
            _trace = Array.IndexOf(args, "--trace") >= 0
                     || Environment.GetEnvironmentVariable("AGENT_TRACE") == "1";
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
                // Ends the STA thread's loop once the request in hand is
                // answered; the listeners are background threads.
                Jobs.CompleteAdding();
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
                // PowerPoint takes enums where the other two take booleans:
                // Visible is an MsoTriState and DisplayAlerts is ppAlertsNone,
                // so the boolean form fails the cast rather than the call.
                var ppt = _app == "powerpoint";
                Settle(() => { if (ppt) app.Visible = -1; else app.Visible = true; }, "Visible");
                Settle(() => { if (ppt) app.DisplayAlerts = 1; else app.DisplayAlerts = false; }, "DisplayAlerts");
                // Serve clients one after another. The first version served
                // exactly one and exited on its disconnect, so the sidecar
                // died the moment anything reconnected -- which is precisely
                // when it should be settling down to wait for the next one.
                // The UIA sidecar learned this already; this one had not.
                //
                // And several at once. One client at a time meant a desktop
                // MCP client that held Excel all day locked the Syn console
                // (and every other client) out of it: the second connection
                // waited for a pipe instance that never came free. Now up to
                // MaxClients listeners each own a pipe instance and hand each
                // request line to this thread, which is still the only one
                // that touches COM. Requests interleave a line at a time,
                // exactly as two people clicking in one Excel would.
                for (var i = 0; i < MaxClients; i++)
                    new Thread(Listen) { IsBackground = true, Name = $"pipe-{i}" }.Start();
                foreach (var job in Jobs.GetConsumingEnumerable())
                {
                    string reply;
                    try { reply = Dispatch(app, job.Line); }
                    catch (Exception e) { reply = Fail($"{e.GetType().Name}: {e.Message}"); }
                    job.Reply.TrySetResult(reply);
                }
            }
            finally
            {
                try { Marshal.FinalReleaseComObject(app); } catch { /* detach only, never kill user app */ }
            }
        }

        /// <summary>One pipe instance: accept a client, pass each line it
        /// sends to the STA thread, write back the reply, repeat.</summary>
        private static void Listen()
        {
            // UTF8Encoding(false): the default Encoding.UTF8 carries a
            // byte-order-mark preamble, and StreamWriter emits it on the first
            // flush -- three stray bytes in front of the first JSON reply,
            // which every client then fails to parse.
            var utf8 = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false);
            while (!_stop)
            {
                NamedPipeServerStream? server = null;
                try
                {
                    // Byte mode, not Message: the client is an ordinary
                    // StreamReader/StreamWriter pair, and a message-mode
                    // server framed against a byte-mode client never
                    // completes a read. Every instance must name the same
                    // MaxClients, or the second one fails to create.
                    server = new NamedPipeServerStream(_pipe, PipeDirection.InOut, MaxClients,
                        PipeTransmissionMode.Byte, PipeOptions.None);
                    Trace($"pipe {_pipe}: waiting for client");
                    server.WaitForConnection();
                    Trace("pipe: client connected");
                    using var reader = new StreamReader(server, utf8, detectEncodingFromByteOrderMarks: false);
                    using var writer = new StreamWriter(server, utf8) { AutoFlush = true };
                    while (!_stop)
                    {
                        var line = reader.ReadLine();
                        if (line == null) { Trace("pipe: EOF, client gone"); break; }
                        Trace($"pipe: got {line.Length} chars");
                        var job = new Job(line);
                        try { Jobs.Add(job); }
                        catch (InvalidOperationException) { break; } // stopping
                        writer.WriteLine(job.Reply.Task.Result);
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
                // `open` comes before the per-app dispatch because it is the
                // one method that must work when the document is NOT open:
                // every other path starts by finding it and fails if it is
                // missing. Until now a run could only drive documents a
                // human had already opened by hand, which made the harness
                // depend on someone sitting at the machine.
                if (method == "open")
                {
                    return OpenDocument(app, JsonField(JsonField(line, "args"), "path"));
                }
                return _app switch
                {
                    "word" => WordDispatch(app, method, handle, line, selector),
                    "excel" => ExcelDispatch(app, method, handle, line, selector),
                    "powerpoint" => PptDispatch(app, method, handle, line, selector),
                    _ => Fail("unknown app"),
                };
            }
            catch (Exception e)
            {
                return Fail(e.Message);
            }
        }

        /// Open a file in this sidecar's application, and show it.
        ///
        /// Idempotent: a document already open is returned as it is rather
        /// than opened twice, because Office answers a second Open of the
        /// same path with a read-only copy and the run would then write
        /// into the copy.
        ///
        /// It never closes anything and never saves anything. The sidecar's
        /// rule is that it does not touch what it did not start, and an
        /// `open` that could clobber the human's unsaved work would break
        /// that in the worst possible way.
        private static string OpenDocument(dynamic app, string path)
        {
            if (string.IsNullOrWhiteSpace(path)) return Fail("open needs a path");
            if (!File.Exists(path)) return Fail($"no such file: {path}");
            var full = Path.GetFullPath(path);
            var name = Path.GetFileName(full);

            return Guarded(() =>
            {
                switch (_app)
                {
                    case "word":
                    {
                        for (var i = 1; i <= (int)app.Documents.Count; i++)
                            if ((string)app.Documents[i].Name == name)
                                return Ok($"already open: {name}");
                        app.Visible = true;
                        app.Documents.Open(full);
                        return Ok($"opened {name}, {(int)app.Documents.Count} document(s)");
                    }
                    case "excel":
                    {
                        for (var i = 1; i <= (int)app.Workbooks.Count; i++)
                            if ((string)app.Workbooks[i].Name == name)
                                return Ok($"already open: {name}");
                        app.Visible = true;
                        app.Workbooks.Open(full);
                        return Ok($"opened {name}, {(int)app.Workbooks.Count} workbook(s)");
                    }
                    case "powerpoint":
                    {
                        for (var i = 1; i <= (int)app.Presentations.Count; i++)
                            if ((string)app.Presentations[i].Name == name)
                                return Ok($"already open: {name}");
                        // MsoTriState, not a boolean: PowerPoint differs
                        // from the other two here and the boolean form
                        // fails the cast rather than the call.
                        app.Visible = -1;
                        app.Presentations.Open(full);
                        return Ok($"opened {name}, {(int)app.Presentations.Count} presentation(s)");
                    }
                    default:
                        return Fail($"open: unknown app {_app}");
                }
            });
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
        private const char CrChar = (char)13;
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

        /// Calls VBA has no business making on this project's behalf.
        ///
        /// A seatbelt, not a cage: string concatenation defeats any list
        /// like this, and anyone enabling VBA should know that. What it
        /// does buy is that careless code -- a model reaching for the
        /// shell because that is what it would do in Python -- is stopped
        /// before it compiles, and stopped with a message saying why.
        private static readonly string[] VbaRefused =
        {
            "Shell", "CreateObject", "GetObject", "FileSystemObject", "WScript",
            "Kill ", "RmDir", "SetAttr", "SendKeys", "URLDownloadToFile",
            "Declare Function", "Declare PtrSafe", "Environ", "Registry",
        };

        /// Entry points Office runs by itself. A macro that can start
        /// itself is not a macro the human approved running.
        private static readonly string[] VbaAutoRun =
        {
            "Auto_Open", "Auto_Close", "Auto_Exec", "Workbook_Open",
            "Document_Open", "AutoExec", "AutoOpen",
        };

        /// Write, run, read or list VBA in the workbook.
        ///
        /// Write-then-verify is not optional here. With "Trust access to
        /// the VBA project object model" off, `VBComponents.Add` returns
        /// **null and raises nothing at all** -- measured on a real
        /// machine, where `VBProject` answered and `.Count` cheerfully
        /// said 0. Trusting the add would report a module that does not
        /// exist, which is the quietest possible failure and exactly the
        /// class of bug this project has spent its time killing.
        private static string Macro(dynamic wb, string action, string module, string name, string code)
        {
            if (string.IsNullOrWhiteSpace(module)) module = "SynMacros";

            // Policy before capability. This used to sit inside the write
            // case, below `wb.VBProject`, which meant a macro calling
            // Shell was refused for the wrong reason on a machine where
            // VBA access happened to be off, and only checked at all on
            // machines where it was on. A refusal that depends on a
            // Trust Center setting is not a policy.
            if (!string.IsNullOrEmpty(code))
            {
                foreach (var bad in VbaRefused)
                    if (code.IndexOf(bad, StringComparison.OrdinalIgnoreCase) >= 0)
                        return Fail($"refused: the code uses {bad.Trim()}, which a generated macro may not call");
                foreach (var bad in VbaAutoRun)
                    if (code.IndexOf(bad, StringComparison.OrdinalIgnoreCase) >= 0)
                        return Fail($"refused: {bad} runs by itself, so it would execute without anyone asking");
            }

            dynamic? proj;
            try
            {
                proj = wb.VBProject;
            }
            catch (Exception e)
            {
                return Fail($"no VBA project: {e.Message}. Office needs Trust Center > Macro Settings > Trust access to the VBA project object model");
            }
            if (proj is null) return Fail("no VBA project on this workbook");

            switch (action.ToLowerInvariant())
            {
                case "list":
                {
                    var names = new List<string>();
                    for (var i = 1; i <= (int)proj.VBComponents.Count; i++)
                        names.Add((string)proj.VBComponents.Item(i).Name);
                    return Ok(names.Count == 0 ? "no modules" : $"modules: {string.Join(", ", names)}");
                }
                case "read":
                {
                    dynamic? c = FindComponent(proj, module);
                    if (c is null) return Fail($"no module named {module}");
                    int lines = c.CodeModule.CountOfLines;
                    return Ok(lines == 0 ? $"{module} is empty" : $"{module}:\n{c.CodeModule.Lines(1, lines)}");
                }
                case "run":
                {
                    if (string.IsNullOrWhiteSpace(name)) return Fail("run needs the macro name in `title`");
                    // Before the macro, never after: running VBA clears
                    // Excel's undo stack, so this copy is the only way
                    // back from a macro that does the wrong thing.
                    var saved = BackupWorkbook(wb);
                    var note = saved.Length > 0 ? $" (copy at {saved})" : " (NO BACKUP: the copy failed)";
                    try
                    {
                        wb.Application.Run(name);
                        return Ok($"ran {name}{note}");
                    }
                    catch (Exception e)
                    {
                        // The error IS the product: this is the half of
                        // the write-run-fix loop that teaches the model
                        // anything, so it comes back whole rather than as
                        // "macro failed".
                        return Fail($"{name} raised: {e.Message}{note}");
                    }
                }
                case "write":
                {
                    if (string.IsNullOrWhiteSpace(code)) return Fail("write needs `code`");
                    dynamic? c = FindComponent(proj, module);
                    if (c is null)
                    {
                        c = proj.VBComponents.Add(1); // vbext_ct_StdModule
                        if (c is null)
                            return Fail("VBComponents.Add returned nothing: Trust Center > Macro Settings > Trust access to the VBA project object model is off");
                        c.Name = module;
                    }
                    else if ((int)c.CodeModule.CountOfLines > 0)
                    {
                        // A write replaces the module, so iterating on a
                        // macro does not stack four copies of it.
                        c.CodeModule.DeleteLines(1, (int)c.CodeModule.CountOfLines);
                    }
                    c.CodeModule.AddFromString(code);

                    // Read it back. Never report a module on the strength
                    // of having asked for one.
                    int now = c.CodeModule.CountOfLines;
                    if (now == 0) return Fail($"{module} is still empty after the write: nothing was stored");
                    return Ok($"wrote {module}, {now} line(s). Run it to find out whether it compiles");
                }
                default:
                    return Fail($"macro: unknown action {action}, expected write, run, read or list");
            }
        }

        private static dynamic? FindComponent(dynamic proj, string module)
        {
            for (var i = 1; i <= (int)proj.VBComponents.Count; i++)
            {
                dynamic c = proj.VBComponents.Item(i);
                if ((string)c.Name == module) return c;
            }
            return null;
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
                    "macro" => Macro(wb, JsonField(args, "action"), JsonField(args, "name"),
                                     JsonField(args, "title"), JsonField(line, "payload")),
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

        /// Unbuffered stderr trace, off unless --trace or AGENT_TRACE=1.
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

        // A sheet name with a space is written the way Excel writes it,
        // 'Q3 sales'!A1, and the MCP guidance tells models to write it so.
        // The quotes are syntax, not part of the name: left on, the lookup
        // asked for a sheet called 'Q3 sales' with the quotes and found none.
        // Found by the LibreOffice helper's live test (sidecar-lo), which
        // handles it the same way; a doubled quote inside is one quote.
        private static (string sheet, string addr) SplitRange(string selector)
        {
            var i = selector.IndexOf('!');
            var (sheet, addr) = i < 0 ? (selector, "") : (selector[..i], selector[(i + 1)..]);
            if (sheet.Length >= 2 && sheet[0] == '\'' && sheet[^1] == '\'')
                sheet = sheet[1..^1].Replace("''", "'");
            return (sheet, addr);
        }

        // ---- PowerPoint ----
        private static string PptDispatch(dynamic app, string method, string handle, string line, string selector) =>
            Guarded(() =>
            {
                dynamic pres = FindPresentation(app, handle)
                    ?? throw new InvalidOperationException($"presentation not open for {handle}");
                var args = JsonField(line, "args");
                return method switch
                {
                    "read" when selector is "deck" or "" => Ok(ReadDeck(pres)),
                    "read" => Ok(ReadSlide(pres, selector)),
                    "write" => WriteSlide(pres, handle, selector, JsonField(line, "payload")),
                    "createSlide" => CreateSlide(pres, handle, JsonField(args, "title"),
                                                 JsonField(line, "payload"), JsonField(args, "name")),
                    "insertTable" => SlideTable(pres, handle, selector, JsonField(line, "payload"),
                                                JsonField(args, "name")),
                    "picture" => SlidePicture(pres, handle, selector, JsonField(args, "text"),
                                              JsonField(args, "name")),
                    "pageNumbers" => SlideNumbers(pres, handle, JsonField(args, "text")),
                    "format" => FormatSlide(pres, handle, selector, JsonField(line, "payload")),
                    "export" => ExportPres(pres, handle, JsonField(line, "format"), JsonField(line, "path")),
                    _ => throw new InvalidOperationException($"unsupported ppt.{method} sel={selector}"),
                };
            });

        // By name, like the workbook and the document. ActivePresentation is
        // whatever the human last clicked on, which is not the same thing as
        // the handle the caller asked for.
        private static dynamic? FindPresentation(dynamic app, string handle)
        {
            int n = app.Presentations.Count;
            for (var i = 1; i <= n; i++)
            {
                dynamic pres = app.Presentations[i];
                string name = pres.Name;
                if (handle.Contains(name)) return pres;
            }
            return null;
        }

        // A slide selector is "s3": the third slide, one-based, as anyone
        // counting slides would say it. "s3.notes" is its speaker notes.
        private static (object slide, bool notes) SlideOf(object presObj, string selector)
        {
            dynamic pres = presObj;
            var sel = selector.Trim();
            var notes = false;
            if (sel.EndsWith(".notes", StringComparison.OrdinalIgnoreCase))
            {
                notes = true;
                sel = sel[..^6];
            }
            if (!sel.StartsWith("s", StringComparison.OrdinalIgnoreCase) || !int.TryParse(sel[1..], out var n))
                throw new InvalidOperationException($"a slide selector looks like s3, not {selector}");
            int count = pres.Slides.Count;
            if (n < 1 || n > count)
                throw new InvalidOperationException($"slide {n} does not exist: the deck has {count}");
            return (pres.Slides[n], notes);
        }

        private static string ReadDeck(dynamic pres)
        {
            var sb = new StringBuilder();
            int n = pres.Slides.Count;
            sb.Append($"slides={n}");
            for (var i = 1; i <= n && i <= 40; i++)
            {
                string title = "";
                try { title = pres.Slides[i].Shapes.Title.TextFrame.TextRange.Text; } catch { }
                sb.Append($" | s{i}: {Trunc(title)}");
            }
            return sb.ToString();
        }

        private static string ReadSlide(dynamic pres, string selector)
        {
            var found = SlideOf((object)pres, selector);
            dynamic slide = found.slide;
            var notes = found.notes;
            if (notes)
            {
                try { return $"notes: {Trunc((string)slide.NotesPage.Shapes.Placeholders[2].TextFrame.TextRange.Text)}"; }
                catch { return "notes: (none)"; }
            }
            var sb = new StringBuilder();
            int n = slide.Shapes.Count;
            sb.Append($"shapes={n}");
            for (var i = 1; i <= n; i++)
            {
                dynamic sh = slide.Shapes[i];
                string t = "";
                // MsoTriState again: msoTrue is -1, and treating it as a bool
                // threw into the catch below, so every slide read as empty.
                try
                {
                    if ((int)sh.HasTextFrame != 0 && (int)sh.TextFrame.HasText != 0)
                        t = sh.TextFrame.TextRange.Text;
                }
                catch { }
                if (t != "") sb.Append($" | {Trunc(t)}");
            }
            return sb.ToString();
        }

        // ppLayout indices on the default master. Named, because "2" in a
        // script tells the next reader nothing.
        private static int LayoutIndex(string name) => (name ?? "").Trim().ToLowerInvariant() switch
        {
            "" or "titlecontent" or "content" => 2,
            "title" or "titleslide" => 1,
            "section" or "sectionheader" => 3,
            "two" or "twocontent" => 4,
            "comparison" => 5,
            "titleonly" => 6,
            "blank" => 7,
            _ => throw new InvalidOperationException(
                    $"createSlide does not know layout {name}: it knows title, titleContent, "
                    + "sectionHeader, twoContent, comparison, titleOnly, blank"),
        };

        private static string CreateSlide(dynamic pres, string handle, string title, string bullets, string layout)
        {
            Snapshot(handle);
            int want = LayoutIndex(layout);
            dynamic layouts = pres.SlideMaster.CustomLayouts;
            if (want > layouts.Count) want = 2;
            dynamic slide = pres.Slides.AddSlide(pres.Slides.Count + 1, layouts[want]);

            if (!string.IsNullOrWhiteSpace(title))
            {
                try { slide.Shapes.Title.TextFrame.TextRange.Text = title; }
                catch { /* a blank layout has no title placeholder */ }
            }

            var added = 0;
            // Bullets ride as one row of the same escaped grid the sheet and
            // the Word table use, so there is one splitting rule in the
            // engine rather than three.
            var rows = ParseGrid(bullets ?? "");
            if (rows.Length > 0 && rows[0].Length > 0 && !(rows[0].Length == 1 && rows[0][0] == ""))
            {
                dynamic? body = null;
                int np = slide.Shapes.Placeholders.Count;
                for (var i = 1; i <= np; i++)
                {
                    dynamic ph = slide.Shapes.Placeholders[i];
                    // ppPlaceholderTitle 1, ppPlaceholderCenterTitle 3 -- skip
                    // those; the first other one that takes text is the body.
                    int t = ph.PlaceholderFormat.Type;
                    if (t == 1 || t == 3) continue;
                    if ((int)ph.HasTextFrame == 0) continue;
                    body = ph;
                    break;
                }
                if (body == null)
                {
                    // ppLayoutBlank and titleOnly have nowhere to put text, so
                    // make somewhere rather than dropping the content.
                    body = slide.Shapes.AddTextbox(1, 60.0, 140.0, 600.0, 320.0);
                }
                // A leading ">" marks a sub-bullet, one per level. Leading
                // spaces would be the obvious marker, but the grid splitter
                // trims every cell -- correctly, for a spreadsheet -- so they
                // never survive the trip.
                var text = new StringBuilder();
                var levels = new List<int>();
                foreach (var b in rows[0])
                {
                    var depth = 0;
                    var body2 = b;
                    while (body2.StartsWith('>'))
                    {
                        depth++;
                        body2 = body2[1..].TrimStart();
                    }
                    if (text.Length > 0) text.Append(CrChar);
                    text.Append(body2);
                    levels.Add(Math.Min(depth + 1, 5));
                    added++;
                }
                body.TextFrame.TextRange.Text = text.ToString();
                for (var i = 0; i < levels.Count; i++)
                {
                    if (levels[i] == 1) continue;
                    // Paragraphs(start, length): without the length it runs to
                    // the end of the range and indents everything after it too.
                    try { body.TextFrame.TextRange.Paragraphs(i + 1, 1).IndentLevel = levels[i]; }
                    catch { }
                }
            }
            return Ok($"slide {pres.Slides.Count} added, layout {layout}, {added} bullet(s)");
        }

        private static string WriteSlide(dynamic pres, string handle, string selector, string text)
        {
            Snapshot(handle);
            var found = SlideOf((object)pres, selector);
            dynamic slide = found.slide;
            var notes = found.notes;
            if (notes)
            {
                slide.NotesPage.Shapes.Placeholders[2].TextFrame.TextRange.Text = text ?? "";
                return Ok($"notes on {selector} written ({(text ?? "").Length} chars)");
            }
            // Without ".notes" this is the title: the body is what createSlide
            // fills, and overwriting it from here would silently drop bullets.
            slide.Shapes.Title.TextFrame.TextRange.Text = text ?? "";
            return Ok($"title on {selector} written ({(text ?? "").Length} chars)");
        }

        private static string SlideTable(dynamic pres, string handle, string selector, string grid, string at)
        {
            Snapshot(handle);
            dynamic slide = SlideOf((object)pres, selector).slide;
            var rows = ParseGrid(grid);
            if (rows.Length == 0) throw new InvalidOperationException("insertTable needs rows: cells by |, rows by ;");
            int nc = 0;
            foreach (var r in rows) nc = Math.Max(nc, r.Length);
            var (l, t, w, h) = Box(at, 50.0, 140.0, 620.0, Math.Min(340.0, 30.0 * rows.Length));
            dynamic shape = slide.Shapes.AddTable(rows.Length, nc, l, t, w, h);
            dynamic table = shape.Table;
            for (var r = 0; r < rows.Length; r++)
                for (var c = 0; c < nc; c++)
                    table.Cell(r + 1, c + 1).Shape.TextFrame.TextRange.Text =
                        c < rows[r].Length ? rows[r][c] : "";
            return Ok($"table on {selector}, {rows.Length}x{nc}");
        }

        private static string SlidePicture(dynamic pres, string handle, string selector, string path, string at)
        {
            Snapshot(handle);
            dynamic slide = SlideOf((object)pres, selector).slide;
            var full = Path.GetFullPath(path);
            if (!File.Exists(full)) throw new InvalidOperationException($"no picture at {full}");
            var (l, t, w, h) = Box(at, 60.0, 130.0, 600.0, 340.0);
            // msoFalse 0, msoTrue -1: do not link, do save with the document.
            dynamic pic = slide.Shapes.AddPicture(full, 0, -1, l, t, w, h);
            try { pic.LockAspectRatio = -1; } catch { }
            return Ok($"picture on {selector} from {Path.GetFileName(full)} at {l},{t} {w}x{h}");
        }

        // "left,top,width,height" in points, any of them omitted. A deck that
        // cannot say where things go is a deck of overlapping shapes, which is
        // the mistake the Excel dashboard already made once.
        private static (double l, double t, double w, double h) Box(
            string spec, double dl, double dt, double dw, double dh)
        {
            if (string.IsNullOrWhiteSpace(spec)) return (dl, dt, dw, dh);
            var parts = spec.Split(',');
            double Pick(int i, double fallback) =>
                i < parts.Length && double.TryParse(parts[i].Trim(), NumberStyles.Any,
                    CultureInfo.InvariantCulture, out var v) ? v : fallback;
            return (Pick(0, dl), Pick(1, dt), Pick(2, dw), Pick(3, dh));
        }

        private static string SlideNumbers(dynamic pres, string handle, string text)
        {
            Snapshot(handle);
            dynamic hf = pres.SlideMaster.HeadersFooters;
            hf.SlideNumber.Visible = true;
            if (!string.IsNullOrWhiteSpace(text))
            {
                hf.Footer.Visible = true;
                hf.Footer.Text = text;
            }
            // The master governs new slides; the ones already placed each
            // carry their own copy and do not inherit retroactively.
            int n = pres.Slides.Count;
            var done = 0;
            for (var i = 1; i <= n; i++)
            {
                try
                {
                    dynamic sf = pres.Slides[i].HeadersFooters;
                    sf.SlideNumber.Visible = true;
                    if (!string.IsNullOrWhiteSpace(text)) { sf.Footer.Visible = true; sf.Footer.Text = text; }
                    done++;
                }
                catch { }
            }
            return Ok($"slide numbers on, footer set on {done} of {n} slides");
        }

        private static string FormatSlide(dynamic pres, string handle, string selector, string styles)
        {
            Snapshot(handle);
            dynamic slide = SlideOf((object)pres, selector).slide;
            dynamic range = slide.Shapes.Title.TextFrame.TextRange;
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
                    case "bold": range.Font.Bold = on ? -1 : 0; break;
                    case "italic": range.Font.Italic = on ? -1 : 0; break;
                    case "size": range.Font.Size = double.Parse(val, CultureInfo.InvariantCulture); break;
                    case "font": range.Font.Name = val; break;
                    case "color": range.Font.Color.RGB = OleColor(val); break;
                    default:
                        throw new InvalidOperationException(
                            $"slide format does not know {key}: it knows bold, italic, size, font, color");
                }
                did.Add(key);
            }
            if (did.Count == 0) throw new InvalidOperationException("format was given no style: try size=32;bold=1");
            return Ok($"formatted the title of {selector}: {string.Join(", ", did)}");
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
                var dir = Path.Combine(Path.GetTempPath(), "agent-snaps");
                Directory.CreateDirectory(dir);
                File.WriteAllText(Path.Combine(dir, safe + "." + DateTime.UtcNow.Ticks + ".bak"), handle);
            }
            catch { /* snapshot is best-effort; the op still reports */ }
        }

        /// A real copy of the workbook, not a marker.
        ///
        /// `Snapshot` above writes the handle into a `.bak` file: enough to
        /// trace that an op happened, and worth nothing if you need the
        /// data back. That is a fair trade for a cell write, which the
        /// application's own undo stack covers. It is not a fair trade
        /// before running VBA, because a macro can do anything and,
        /// worse, **running one clears Excel's undo stack** -- so the
        /// cheap safety net that makes the marker acceptable everywhere
        /// else is precisely what is gone here.
        ///
        /// Returns the path, or an empty string if the copy failed. The
        /// caller reports it: a human deciding whether to run generated
        /// code should know whether there is anything to go back to.
        private static string BackupWorkbook(dynamic wb)
        {
            try
            {
                var dir = Path.Combine(Path.GetTempPath(), "agent-snaps");
                Directory.CreateDirectory(dir);
                var name = (string)wb.Name;
                var to = Path.Combine(dir, $"{Path.GetFileNameWithoutExtension(name)}.{DateTime.UtcNow.Ticks}{Path.GetExtension(name)}");
                wb.SaveCopyAs(to);
                return to;
            }
            catch
            {
                return "";
            }
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
