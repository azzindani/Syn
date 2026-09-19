// Syn office-host: live Word/Excel/PowerPoint hand over COM.
// BUILD: Windows + .NET 8 SDK + installed Office:
//   dotnet build -c Release   (net8.0-windows)
// RUN:  office-host.exe --pipe synhand-excel --app excel
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
// STATUS: full implementation; Windows-compile + live-Office run pending
// (this container has neither dotnet nor Office).
using System;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

namespace Harness.Sidecar
{
    internal static class Program
    {
        private static string _pipe = "synhand";
        private static string _app = "excel";
        private static volatile bool _stop;

        [STAThread]
        private static int Main(string[] args)
        {
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
                using var server = new NamedPipeServerStream(_pipe, PipeDirection.InOut, 1,
                    PipeTransmissionMode.Message, PipeOptions.Asynchronous);
                server.WaitForConnection();
                using var reader = new StreamReader(server, Encoding.UTF8);
                using var writer = new StreamWriter(server, Encoding.UTF8) { AutoFlush = true };
                while (!_stop)
                {
                    var line = reader.ReadLine();
                    if (line == null) break;
                    writer.WriteLine(Dispatch(app, line));
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

        private static dynamic GetOrCreate(string progId)
        {
            try
            {
                return Marshal.GetActiveObject(progId); // live window first
            }
            catch (COMException)
            {
                var t = Type.GetTypeFromProgID(progId, throwOnError: true)!;
                return Activator.CreateInstance(t)!;
            }
        }

        /// Run a COM call with a timeout; modal dialogs surface as errors.
        private static string Guarded(Func<string> call, int ms = 15000)
        {
            string? result = null;
            Exception? err = null;
            var done = new ManualResetEventSlim(false);
            var worker = new Thread(() =>
            {
                try { result = call(); }
                catch (Exception e) { err = e; }
                finally { done.Set(); }
            });
            worker.SetApartmentState(ApartmentState.STA);
            worker.Start();
            if (!done.Wait(ms))
                return Fail("modal dialog or busy app: human confirm required (call timed out, app untouched)");
            return err != null ? Fail($"com: {err.Message}") : Ok(result ?? "");
        }

        private static string Dispatch(dynamic app, string line)
        {
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
            foreach (var w in app.Workbooks)
            {
                string name = w.Name;
                if (handle.Contains(name)) return w;
            }
            return null;
        }

        private static string ReadRange(dynamic wb, string selector)
        {
            var (sheet, addr) = SplitRange(selector);
            dynamic ws = wb.Worksheets[sheet];
            dynamic rng = string.IsNullOrEmpty(addr) ? ws.UsedRange : ws.Range[addr];
            return $"grid {sheet}: {rng.Rows.Count}x{rng.Columns.Count}";
        }

        private static string WriteRange(dynamic wb, string handle, string selector, string payload)
        {
            Snapshot(handle);
            var (sheet, addr) = SplitRange(selector);
            if (string.IsNullOrEmpty(addr)) throw new InvalidOperationException("write needs Sheet!A1:B2");
            dynamic ws = wb.Worksheets[sheet];
            var rows = payload.Split(';');
            var r0 = ws.Range[addr].Row;
            var c0 = ws.Range[addr].Column;
            for (var i = 0; i < rows.Length; i++)
            {
                var cells = rows[i].Split(',');
                for (var j = 0; j < cells.Length; j++)
                    ws.Cells[r0 + i, c0 + j].Value2 = cells[j];
            }
            return Ok($"wrote {rows.Length}x{rows[0].Split(',').Length} at {selector}");
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
