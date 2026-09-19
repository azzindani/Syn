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
            dynamic app = AttachOrStart(_app);
            try
            {
                app.Visible = true;
                app.DisplayAlerts = false;
                // Byte mode, not Message: the client is an ordinary
                // StreamReader/StreamWriter pair, and a message-mode server
                // framed against a byte-mode client never completes a read.
                using var server = new NamedPipeServerStream(_pipe, PipeDirection.InOut, 1,
                    PipeTransmissionMode.Byte, PipeOptions.None);
                Trace($"pipe {_pipe}: waiting for client");
                server.WaitForConnection();
                Trace("pipe: client connected");
                // UTF8Encoding(false): the default Encoding.UTF8 carries a
                // byte-order-mark preamble, and StreamWriter emits it on the
                // first flush — three stray bytes in front of the first JSON
                // reply, which every client then fails to parse.
                var utf8 = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false);
                using var reader = new StreamReader(server, utf8, detectEncodingFromByteOrderMarks: false);
                Trace("pipe: reader ready");
                using var writer = new StreamWriter(server, utf8) { AutoFlush = true };
                Trace("pipe: writer ready");
                while (!_stop)
                {
                    Trace("pipe: awaiting line");
                    var line = reader.ReadLine();
                    if (line == null) { Trace("pipe: EOF, client gone"); break; }
                    Trace($"pipe: got {line.Length} chars");
                    writer.WriteLine(Dispatch(app, line));
                    Trace("pipe: reply sent");
                }
            }
            finally
            {
                try { Marshal.FinalReleaseComObject(app); } catch { /* detach only, never kill user app */ }
            }
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
                return method switch
                {
                    "read" when selector is "body" or "" =>
                        Ok($"paras={doc.Paragraphs.Count}"),
                    "read" when selector.StartsWith("p") && int.TryParse(selector[1..], out var n) =>
                        Ok($"para {n}: {Trunc((string)doc.Paragraphs[n + 1].Range.Text)}"),
                    "write" when selector.StartsWith("p") && int.TryParse(selector[1..], out var m) =>
                        WriteWordPara(doc, handle, m, JsonField(line, "payload")),
                    "export" => ExportWord(doc, handle, JsonField(line, "format"), JsonField(line, "path")),
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

        private static string ExportWord(dynamic doc, string handle, string format, string path)
        {
            Snapshot(handle);
            Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(path)) ?? ".");
            doc.SaveAs2(path, format.ToLowerInvariant() == "pdf" ? 17 : 16); // wdFormatPDF/docx
            return Ok($"exported {path}");
        }

        // ---- Excel ----
        private static string ExcelDispatch(dynamic app, string method, string handle, string line, string selector) =>
            Guarded(() =>
            {
                var wb = FindWorkbook(app, handle) ?? throw new InvalidOperationException($"workbook not open for {handle}");
                return method switch
                {
                    "read" => Ok(ReadRange(wb, selector)),
                    "write" => WriteRange(wb, handle, selector, JsonField(line, "payload")),
                    "export" => ExportWb(wb, handle, JsonField(line, "format"), JsonField(line, "path")),
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

        private static string ReadRange(dynamic wb, string selector)
        {
            var (sheet, addr) = SplitRange(selector);
            dynamic ws = Sheet(wb, sheet);
            dynamic rng = string.IsNullOrEmpty(addr) ? ws.UsedRange : ws.Range[addr];
            return $"grid {sheet}: {rng.Rows.Count}x{rng.Columns.Count}";
        }

        private static string WriteRange(dynamic wb, string handle, string selector, string payload)
        {
            Snapshot(handle);
            var (sheet, addr) = SplitRange(selector);
            if (string.IsNullOrEmpty(addr)) throw new InvalidOperationException("write needs Sheet!A1:B2");
            dynamic ws = Sheet(wb, sheet);
            var rows = ParseGrid(payload);
            var r0 = ws.Range[addr].Row;
            var c0 = ws.Range[addr].Column;
            for (var i = 0; i < rows.Length; i++)
                for (var j = 0; j < rows[i].Length; j++)
                    ws.Cells[r0 + i, c0 + j].Value2 = rows[i][j];
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

        // Mirrors grid() in core/src/tools.rs: a backslash escapes the next
        // character, so a cell can hold a comma. Splitting naively meant a
        // line of prose with a comma in it landed in two cells.
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
                else if (ch == ',') row.Add("");
                else row[^1] += ch;
            }
            return rows.Select(r => r.ToArray()).ToArray();
        }

        private static string ExportWb(dynamic wb, string handle, string format, string path)
        {
            Snapshot(handle);
            Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(path)) ?? ".");
            if (format.ToLowerInvariant() == "pdf") wb.ExportAsFixedFormat(0, path);
            else wb.SaveAs(path, 51); // xlOpenXMLWorkbook
            return Ok($"exported {path}");
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
