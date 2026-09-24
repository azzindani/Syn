using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.IO.Pipes;
using System.Linq;
using System.Text;
using System.Threading;
using System.Windows.Automation;

// uia-host: the UI Automation hand.
//
// Same wire protocol as office-host (office-rpc/1, one JSON object per line,
// byte-mode named pipe, UTF-8 with no BOM), so core reaches it through the
// existing `Hand` with no new transport. The protocol is the seam; nothing
// about UIA leaks into Rust.
//
// Why this hand matters: it is the answer to "what about apps with no API".
// UIA exposes a tree of elements with names, roles and values, so a model
// reads STRUCTURE, not pixels, and presses a control by identity rather than
// by coordinate. A window that moves does not break it, and reading the
// screen costs no image tokens.
//
// Handles are `ui:<window match>:<unit>`:
//   window match - substring of the window title, or `pid=1234`
//   unit         - a selector naming the region, or `:self` for the window
// An op's own selector then resolves inside that unit, exactly as a range
// resolves inside a sheet.
//
// Selector language, deliberately tiny and closed: comma-separated `k=v`
// pairs, ALL of which must match.
//   name=Save        Name equals, else contains (case-insensitive)
//   id=num5Button    AutomationId, exact
//   type=Button      ControlType
//   class=Edit       ClassName
// `:self` selects the unit element itself. Anything else is rejected rather
// than guessed at, because a selector that silently matches the wrong
// control presses the wrong button.

namespace Syn.Uia
{
    internal static class Program
    {
        private static string _pipe = "hand-uia";
        private static bool _trace;
        private static volatile bool _stop;

        /// <summary>How deep a tree dump may walk. A full desktop tree is
        /// tens of thousands of elements and would bury the caller.</summary>
        private const int MaxDepth = 6;
        private const int MaxNodes = 300;

        // UI Automation clients must NOT be STA. A client that pumps on an
        // STA thread can deadlock against an STA provider it is calling into;
        // Microsoft's guidance is an MTA client. This is the opposite of the
        // Office sidecar, which must be STA because it drives COM servers
        // that require it -- the two hands genuinely need different
        // apartments, which is another reason they are separate processes.
        [MTAThread]
        private static int Main(string[] args)
        {
            _trace = Array.IndexOf(args, "--trace") >= 0
                     || Environment.GetEnvironmentVariable("AGENT_TRACE") == "1";
            for (var i = 0; i + 1 < args.Length; i += 2)
            {
                if (args[i] == "--pipe") _pipe = args[i + 1];
            }
            Console.CancelKeyPress += (_, e) =>
            {
                e.Cancel = true;
                _stop = true;
                Console.WriteLine("stop requested: draining");
            };
            Console.WriteLine($"uia-host live: pipe={_pipe} (MTA)");
            Serve();
            return 0;
        }

        /// <summary>How many clients may be connected at once.</summary>
        private const int MaxClients = 8;

        /// <summary>One request at a time across every client. UI Automation
        /// from an MTA is safe to call from several threads, but a tree walk
        /// racing a click on the same window is not a thing to find out
        /// about live.</summary>
        private static readonly object Gate = new();

        // Many clients over the process lifetime, and several at once. A
        // single-shot server exited when the first CLI session ended, so the
        // next `hand hand-uia` failed with a confusing "no hand"; a
        // one-at-a-time server let a desktop MCP client that held the hand
        // lock every other client out of it. The attached application
        // outlives any one conversation, and is shared by all of them.
        private static void Serve()
        {
            for (var i = 0; i < MaxClients; i++)
                new Thread(Listen) { IsBackground = true, Name = $"pipe-{i}" }.Start();
            while (!_stop) Thread.Sleep(200);
        }

        private static void Listen()
        {
            var utf8 = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false);
            while (!_stop)
            {
                NamedPipeServerStream? server = null;
                try
                {
                    // Every instance must name the same MaxClients, or the
                    // second one fails to create.
                    server = new NamedPipeServerStream(_pipe, PipeDirection.InOut, MaxClients,
                        PipeTransmissionMode.Byte, PipeOptions.None);
                    Trace($"pipe {_pipe}: waiting for client");
                    server.WaitForConnection();
                    Trace("pipe: client connected");
                    var reader = new StreamReader(server, utf8, detectEncodingFromByteOrderMarks: false);
                    var writer = new StreamWriter(server, utf8) { AutoFlush = true };
                    while (!_stop)
                    {
                        var line = reader.ReadLine();
                        if (line == null) { Trace("pipe: EOF, client gone; waiting for the next"); break; }
                        string reply;
                        lock (Gate) reply = Dispatch(line);
                        writer.WriteLine(reply);
                    }
                }
                catch (IOException e)
                {
                    Trace($"pipe: {e.Message}; waiting for the next client");
                }
                finally
                {
                    // Disposing a StreamWriter whose pipe has already gone
                    // throws "Pipe is broken" from the final flush. Letting
                    // that escape kills the sidecar the moment its first
                    // client disconnects -- which is exactly when it should
                    // be settling down to wait for the second one.
                    try { server?.Dispose(); } catch (IOException) { }
                }
            }
        }

        private static string Dispatch(string line)
        {
            Trace($"dispatch in: {line}");
            var method = JsonField(line, "method");
            var handle = JsonField(line, "handle");
            var args = JsonField(line, "args");
            var selector = JsonField(args, "selector");
            if (selector == "") selector = JsonField(line, "selector");
            try
            {
                var unit = UnitOf(handle);
                var window = FindWindow(WindowOf(handle))
                             ?? throw new InvalidOperationException($"no open window matches {WindowOf(handle)}");
                var root = unit is "" or ":self" or ":doc" ? window : Find(window, unit)
                           ?? throw new InvalidOperationException($"unit {unit} not found in that window");

                return method switch
                {
                    "read" when selector == ":tree" => Ok(Tree(root)),
                    "read" => Ok(TextOf(Resolve(root, selector))),
                    "write" => Write(Resolve(root, selector), JsonField(line, "payload")),
                    "invoke" => Invoke(Resolve(root, selector), JsonField(args, "action")),
                    "export" => Export(root, JsonField(args, "format"), JsonField(args, "path")),
                    _ => Fail($"unsupported uia.{method}"),
                };
            }
            catch (Exception e)
            {
                return Fail(e.Message);
            }
        }

        // ---- element lookup ----

        private static string WindowOf(string handle)
        {
            var p = handle.Split(':');
            return p.Length > 1 ? p[1] : "";
        }

        private static string UnitOf(string handle)
        {
            var i = handle.IndexOf(':');
            if (i < 0) return "";
            var j = handle.IndexOf(':', i + 1);
            return j < 0 ? "" : handle[(j + 1)..];
        }

        private static AutomationElement? FindWindow(string match)
        {
            if (match.StartsWith("pid=", StringComparison.OrdinalIgnoreCase)
                && int.TryParse(match[4..], out var pid))
            {
                return AutomationElement.RootElement.FindFirst(TreeScope.Children,
                    new PropertyCondition(AutomationElement.ProcessIdProperty, pid));
            }
            var windows = AutomationElement.RootElement.FindAll(TreeScope.Children, Condition.TrueCondition);
            foreach (AutomationElement w in windows)
            {
                var name = Safe(() => w.Current.Name) ?? "";
                if (name.Length > 0 && name.Contains(match, StringComparison.OrdinalIgnoreCase)) return w;
            }
            return null;
        }

        /// <summary>The op's selector, resolved inside the unit. Empty or
        /// `:self` means the unit element itself.</summary>
        private static AutomationElement Resolve(AutomationElement root, string selector)
        {
            if (selector is "" or ":self") return root;
            return Find(root, selector) ?? throw new InvalidOperationException($"no control matches {selector}");
        }

        private static AutomationElement? Find(AutomationElement root, string selector)
        {
            var conds = new List<Condition>();
            foreach (var part in selector.Split(',', StringSplitOptions.RemoveEmptyEntries))
            {
                var kv = part.Split('=', 2);
                if (kv.Length != 2) throw new InvalidOperationException($"bad selector part '{part}', want k=v");
                var (k, v) = (kv[0].Trim().ToLowerInvariant(), kv[1].Trim());
                switch (k)
                {
                    case "id":
                        conds.Add(new PropertyCondition(AutomationElement.AutomationIdProperty, v));
                        break;
                    case "name":
                        conds.Add(new PropertyCondition(AutomationElement.NameProperty, v));
                        break;
                    case "class":
                        conds.Add(new PropertyCondition(AutomationElement.ClassNameProperty, v));
                        break;
                    case "type":
                        conds.Add(new PropertyCondition(AutomationElement.ControlTypeProperty, ControlTypeNamed(v)));
                        break;
                    default:
                        throw new InvalidOperationException($"unknown selector key '{k}': use name|id|type|class");
                }
            }
            if (conds.Count == 0) return null;
            var cond = conds.Count == 1 ? conds[0] : new AndCondition(conds.ToArray());
            var hit = root.FindFirst(TreeScope.Descendants, cond);
            if (hit != null) return hit;

            // Exact Name found nothing: fall back to a contains scan, but
            // only for name. Names carry punctuation and shortcut hints
            // ("Save As...", "&File") that a caller cannot be expected to
            // reproduce exactly. Never loosened for id: an id is meant to be
            // exact, and a fuzzy id match presses a neighbour.
            var wanted = selector.Split(',')
                .Select(p => p.Split('=', 2))
                .Where(p => p.Length == 2 && p[0].Trim().Equals("name", StringComparison.OrdinalIgnoreCase))
                .Select(p => p[1].Trim())
                .FirstOrDefault();
            if (wanted == null || conds.Count != 1) return null;
            foreach (AutomationElement e in root.FindAll(TreeScope.Descendants, Condition.TrueCondition))
            {
                var n = Safe(() => e.Current.Name) ?? "";
                if (n.Contains(wanted, StringComparison.OrdinalIgnoreCase)) return e;
            }
            return null;
        }

        private static ControlType ControlTypeNamed(string name)
        {
            var f = typeof(ControlType).GetField(name, System.Reflection.BindingFlags.Public
                                                       | System.Reflection.BindingFlags.Static
                                                       | System.Reflection.BindingFlags.IgnoreCase);
            return f?.GetValue(null) as ControlType
                   ?? throw new InvalidOperationException($"unknown control type {name}");
        }

        // ---- ops ----

        private static string TextOf(AutomationElement e)
        {
            if (e.TryGetCurrentPattern(ValuePattern.Pattern, out var v))
                return Trunc(((ValuePattern)v).Current.Value ?? "");
            if (e.TryGetCurrentPattern(TextPattern.Pattern, out var t))
                return Trunc(((TextPattern)t).DocumentRange.GetText(4000));
            return Trunc(Safe(() => e.Current.Name) ?? "");
        }

        private static string Write(AutomationElement e, string text)
        {
            if (!e.TryGetCurrentPattern(ValuePattern.Pattern, out var v))
                throw new InvalidOperationException(
                    "that control exposes no ValuePattern, so its text cannot be set without synthetic keystrokes");
            var vp = (ValuePattern)v;
            if (vp.Current.IsReadOnly) throw new InvalidOperationException("that control is read-only");
            e.SetFocus();
            vp.SetValue(text);
            return Ok($"set {text.Length} chars");
        }

        private static string Invoke(AutomationElement e, string action)
        {
            if (action == "") action = "invoke";
            switch (action)
            {
                case "invoke":
                case "click":
                    if (e.TryGetCurrentPattern(InvokePattern.Pattern, out var i))
                    {
                        ((InvokePattern)i).Invoke();
                        return Ok($"invoked {Describe(e)}");
                    }
                    // A checkbox has no InvokePattern; pressing it is Toggle.
                    if (e.TryGetCurrentPattern(TogglePattern.Pattern, out var t2))
                    {
                        ((TogglePattern)t2).Toggle();
                        return Ok($"toggled {Describe(e)}");
                    }
                    throw new InvalidOperationException($"{Describe(e)} supports neither Invoke nor Toggle");
                case "toggle":
                    Require<TogglePattern>(e, TogglePattern.Pattern, "Toggle").Toggle();
                    return Ok($"toggled {Describe(e)}");
                case "select":
                    Require<SelectionItemPattern>(e, SelectionItemPattern.Pattern, "SelectionItem").Select();
                    return Ok($"selected {Describe(e)}");
                case "expand":
                    Require<ExpandCollapsePattern>(e, ExpandCollapsePattern.Pattern, "ExpandCollapse").Expand();
                    return Ok($"expanded {Describe(e)}");
                case "collapse":
                    Require<ExpandCollapsePattern>(e, ExpandCollapsePattern.Pattern, "ExpandCollapse").Collapse();
                    return Ok($"collapsed {Describe(e)}");
                case "focus":
                    e.SetFocus();
                    return Ok($"focused {Describe(e)}");
                default:
                    throw new InvalidOperationException($"unknown action {action}");
            }
        }

        private static T Require<T>(AutomationElement e, AutomationPattern p, string name) where T : class
        {
            if (e.TryGetCurrentPattern(p, out var got)) return (T)got;
            throw new InvalidOperationException($"{Describe(e)} does not support {name}");
        }

        private static string Export(AutomationElement root, string format, string path)
        {
            var tree = Tree(root);
            if (path == "") return Ok(tree);
            Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(path)) ?? ".");
            File.WriteAllText(path, tree, new UTF8Encoding(false));
            return Ok($"exported {path} ({tree.Length} chars)");
        }

        /// <summary>Indented dump of the control tree: the map a caller needs
        /// before it can address anything.</summary>
        private static string Tree(AutomationElement root)
        {
            var sb = new StringBuilder();
            var n = 0;
            void Walk(AutomationElement e, int depth)
            {
                if (n >= MaxNodes || depth > MaxDepth) return;
                n++;
                var name = Safe(() => e.Current.Name) ?? "";
                var id = Safe(() => e.Current.AutomationId) ?? "";
                var type = Safe(() => e.Current.ControlType.ProgrammaticName) ?? "";
                type = type.Replace("ControlType.", "");
                sb.Append(new string(' ', depth * 2)).Append(type);
                if (id.Length > 0) sb.Append(" id=").Append(id);
                if (name.Length > 0) sb.Append(" name=").Append(Trunc(name, 60));
                sb.Append(" | ");
                foreach (AutomationElement c in e.FindAll(TreeScope.Children, Condition.TrueCondition))
                {
                    if (n >= MaxNodes) break;
                    Walk(c, depth + 1);
                }
            }
            Walk(root, 0);
            if (n >= MaxNodes) sb.Append($"... truncated at {MaxNodes} nodes");
            return sb.ToString();
        }

        private static string Describe(AutomationElement e)
        {
            var name = Safe(() => e.Current.Name) ?? "";
            var id = Safe(() => e.Current.AutomationId) ?? "";
            return name.Length > 0 ? name : (id.Length > 0 ? id : "element");
        }

        // ---- plumbing ----

        /// <summary>A UIA element can vanish between finding it and reading
        /// it; that is an ordinary race, not a failure of the call.</summary>
        private static T? Safe<T>(Func<T> f) where T : class
        {
            try { return f(); }
            catch (ElementNotAvailableException) { return null; }
            catch (Exception) { return null; }
        }

        private static string Trunc(string s, int n = 800) =>
            s.Length <= n ? s : s[..n] + $"... (+{s.Length - n} chars)";

        private static string Ok(string preview) => "{\"ok\":true,\"preview\":\"" + Esc(preview) + "\"}";

        private static string Fail(string error) => "{\"ok\":false,\"error\":\"" + Esc(error) + "\"}";

        private static string Esc(string s) =>
            s.Replace("\\", "\\\\").Replace("\"", "\\\"").Replace("\r", " ").Replace("\n", " ");

        /// <summary>Flat JSON string-field read, matching office-host.</summary>
        private static string JsonField(string json, string name)
        {
            var key = "\"" + name + "\"";
            var i = json.IndexOf(key, StringComparison.Ordinal);
            if (i < 0) return "";
            var j = i + key.Length;
            while (j < json.Length && (json[j] == ' ' || json[j] == ':')) j++;
            if (j >= json.Length) return "";
            if (json[j] == '{')
            {
                var depth = 0;
                var start = j;
                for (; j < json.Length; j++)
                {
                    if (json[j] == '{') depth++;
                    else if (json[j] == '}') { depth--; if (depth == 0) return json[start..(j + 1)]; }
                }
                return "";
            }
            if (json[j] != '"') return "";
            j++;
            var sb = new StringBuilder();
            while (j < json.Length)
            {
                if (json[j] == '\\' && j + 1 < json.Length)
                {
                    j++;
                    sb.Append(json[j] switch { 'n' => '\n', 'r' => '\r', 't' => '\t', var c => c });
                }
                else if (json[j] == '"') return sb.ToString();
                else sb.Append(json[j]);
                j++;
            }
            return "";
        }

        private static void Trace(string msg)
        {
            if (_trace) Console.WriteLine($"[{DateTime.Now:HH:mm:ss.fff}] {msg}");
        }
    }
}
