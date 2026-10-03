// What happens when a call to the application does not come back.
//
// A 50-message run hung four times, for 22 minutes to an hour each. Twice a
// formula comparing every row with every other row (SUMIF with a whole
// column of criteria, 85,000 x 85,000 for each of 58 cells) kept Excel
// calculating, and the write call waited as long as Excel did. Once Excel
// showed its "this workbook has links to other sources" box, which only a
// person can answer, in the middle of an `open`. Each time the whole turn
// waited and a human had to find the cause and kill Excel by hand, which a
// helper must never do: Excel is one process per user, and killing it takes
// the person's own workbooks with it.
//
// COM calls all run on the one STA thread, so that thread cannot help
// itself. The listener threads can: they are the ones waiting for the reply,
// so they watch the call, and they only use Win32 (no COM into a busy Excel).
//
//   1. A box the application is waiting on is read (text and buttons) and
//      reported. The one box that is safe to answer for the caller, "update
//      links?", is answered "Don't Update": opening never updates links.
//   2. A call that runs past a limit has Esc pressed at the application,
//      which stops a calculation the way it does for a person. What the call
//      changed is then put back, because the formula that caused it is still
//      there and would start the calculation again.
//   3. A call still not back after a grace period is answered anyway, with
//      what is known, so the caller is told instead of waiting.
//
// Nothing here kills a process or presses a button other than that one.

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        /// <summary>Seconds a quick call (read, write, format, a struct verb)
        /// may run before Esc is pressed. Quick, here, means nothing a person
        /// would wait minutes for: a sort of a hundred thousand rows is
        /// seconds, and a pivot over half a million rows is under one.</summary>
        private static readonly int CallSecs = EnvSecs("AGENT_OFFICE_CALL_SECS", 120);

        /// <summary>The same for calls that are slow by nature: opening a
        /// file, exporting, saving, closing.</summary>
        private static readonly int LongSecs = EnvSecs("AGENT_OFFICE_LONG_SECS", 1200);

        /// <summary>After Esc, how long to wait before answering the caller
        /// with an error and leaving the call to finish on its own.</summary>
        private const int GraceSecs = 60;

        private static readonly HashSet<string> LongMethods = new(StringComparer.Ordinal)
        {
            "open", "export", "save", "close", "saveAs",
        };

        private static int EnvSecs(string name, int fallback) =>
            int.TryParse(Environment.GetEnvironmentVariable(name), out var v) && v is >= 5 and <= 86400 ? v : fallback;

        /// <summary>Counts every undo entry recorded, so a call can tell
        /// whether it recorded one: the stack's size cannot say, as it is
        /// capped.</summary>
        private static int _pushSeq;

        /// <summary>Wait for the reply to one request, watching the
        /// application meanwhile. Always returns a reply line.</summary>
        private static string Await(Job job)
        {
            var method = JsonField(job.Line, "method");
            var longCall = LongMethods.Contains(method);
            var waiting = Stopwatch.StartNew();
            long dialogSince = 0, lastEsc = 0;
            var escapes = 0;
            var tried = new HashSet<string>();
            while (true)
            {
                if (job.Reply.Task.Wait(1000)) return job.Reply.Task.Result;

                if (waiting.Elapsed.TotalSeconds >= 3)
                {
                    BlockingDialog? box = null;
                    try { box = FindBlockingDialog(); }
                    catch (Exception e) { Trace($"watchdog: dialog check failed: {e.Message}"); }
                    if (box != null)
                    {
                        // The one box answered for the caller. Once per box: if
                        // pressing did nothing, it is reported like any other.
                        if (box.IsLinksPrompt && tried.Add(box.Key) && box.Press("Don't Update"))
                        {
                            Console.WriteLine($"{_app}: answered Don't Update to its links box");
                            continue;
                        }
                        if (dialogSince == 0) dialogSince = Environment.TickCount64;
                        if (Environment.TickCount64 - dialogSince >= 10_000) return Abandon(job, box.Describe(_app));
                    }
                    else dialogSince = 0;
                }

                if (!job.Started)
                {
                    // Behind a call that has not returned. Nothing of this one
                    // has run, so it can be refused outright and skipped.
                    if (waiting.Elapsed.TotalSeconds > CallSecs + GraceSecs) return Abandon(job, BusyMessage());
                    continue;
                }

                var running = (Environment.TickCount64 - job.StartTick) / 1000.0;
                var limit = longCall ? LongSecs : CallSecs;
                if (running <= limit) continue;
                if (!longCall && _app == "excel" && (escapes == 0 || Environment.TickCount64 - lastEsc >= 15_000))
                {
                    // Excel stops a calculation on Esc, as for a person at the
                    // keyboard. A key press is the only way in: the STA thread
                    // is busy, and a COM call from here would wait with it.
                    escapes++;
                    lastEsc = Environment.TickCount64;
                    job.Interrupted = true;
                    PressEscape();
                    Console.WriteLine($"{_app}: call running {running:n0} s; pressed Esc ({escapes})");
                }
                if (running > limit + GraceSecs) return Abandon(job, StuckMessage(method, running, longCall));
            }
        }

        /// <summary>Answer the caller now, and see to it that whatever the
        /// call goes on to do is put right when it finally returns.</summary>
        private static string Abandon(Job job, string why)
        {
            lock (job)
            {
                job.Abandoned = true;
                if (job.Started) job.Interrupted = true;
            }
            Console.WriteLine($"{_app}: gave up waiting: {why}");
            return Fail(why);
        }

        private static string BusyMessage() =>
            $"{AppTitle()} is still busy with an earlier request (it has not answered in over {CallSecs + GraceSecs} s), " +
            "so this one was not run. Nothing was changed. Wait a minute and ask again; if it stays busy, " +
            $"look at {AppTitle()}: it may be showing a box that needs an answer.";

        private static string StuckMessage(string method, double secs, bool longCall) =>
            longCall
                ? $"{method} is taking longer than {LongSecs / 60} minutes and {AppTitle()} has still not answered. " +
                  "It may yet finish: read the document before trying it again."
                : $"{AppTitle()} has not answered {method} in {secs:n0} s and did not stop when Esc was pressed. " +
                  "It is probably still calculating a formula that compares every row with every other row. " +
                  "Syn will undo the change as soon as Excel answers; until then do not send more formulas. " +
                  "When it does answer, use a helper column, SUMIFS over a smaller range, or a pivot instead.";

        private static string AppTitle() => _app switch { "word" => "Word", "powerpoint" => "PowerPoint", _ => "Excel" };

        /// <summary>Runs on the STA thread, after a call that was stopped or
        /// given up on has finally returned. Puts back what it changed and
        /// says why, instead of leaving the formula that caused it to start
        /// the same calculation again.</summary>
        private static string Recover(dynamic app, Job job, string reply)
        {
            var secs = (Environment.TickCount64 - job.StartTick) / 1000.0;
            var method = JsonField(job.Line, "method");
            var handle = JsonField(job.Line, "handle");
            // A call that finished by itself, and left Excel done, was not
            // interrupted: Esc arrived as it ended. Keep its result.
            if (!job.Abandoned && IsOk(reply) && !ExcelUnfinished((object)app)) return reply;

            string undone = "";
            object? previous = null;
            try
            {
                previous = app.Calculation;
                app.Calculation = -4135; // xlCalculationManual: the formulas are about to change
            }
            catch (Exception e) { Trace($"recover: manual calculation refused: {e.Message}"); }
            try
            {
                if (_pushSeq != job.PushSeq)
                {
                    var r = UndoLast((object)app, handle);
                    undone = IsOk(r)
                        ? " What it changed was put back."
                        : $" It could not be put back automatically ({JsonField(r, "error")}): undo or remove what it wrote.";
                }
            }
            catch (Exception e) { undone = $" It could not be put back automatically ({e.Message}): undo or remove what it wrote."; }
            try { if (previous != null) app.Calculation = previous; }
            catch (Exception e) { Trace($"recover: calculation mode not restored: {e.Message}"); }

            return Fail(
                $"{method} kept Excel busy for {secs:n0} s, so Syn stopped it (Esc).{undone} " +
                "That is what a formula that compares every row with every other row does " +
                "(SUMIF or COUNTIF with a whole column of criteria, SUMPRODUCT over big ranges, " +
                "or a lookup in every row of a big table). Use a helper column, SUMIFS or COUNTIFS " +
                "on a smaller range, or a pivot.");
        }

        private static bool ExcelUnfinished(object appO)
        {
            try { dynamic app = appO; return (int)app.CalculationState != 0; } // xlDone
            catch { return false; }
        }

        // --- Win32: windows, keys, and the boxes an application shows -------------

        private delegate bool WdEnumProc(IntPtr h, IntPtr l);
        [DllImport("user32.dll", EntryPoint = "EnumWindows")] private static extern bool WdEnumWindows(WdEnumProc cb, IntPtr l);
        [DllImport("user32.dll", EntryPoint = "GetWindowThreadProcessId")] private static extern uint WdGetPid(IntPtr h, out uint pid);
        [DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "GetClassName")] private static extern int WdGetClass(IntPtr h, StringBuilder s, int n);
        [DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "GetWindowText")] private static extern int WdGetText(IntPtr h, StringBuilder s, int n);
        [DllImport("user32.dll", EntryPoint = "IsWindowVisible")] private static extern bool WdIsVisible(IntPtr h);
        [DllImport("user32.dll", EntryPoint = "PostMessage")] private static extern bool WdPost(IntPtr h, uint msg, IntPtr w, IntPtr l);
        [DllImport("user32.dll", EntryPoint = "GetWindowRect")] private static extern bool WdGetRect(IntPtr h, out Rect r);

        [StructLayout(LayoutKind.Sequential)] private struct Rect { public int L, T, R, B; }
        [StructLayout(LayoutKind.Sequential)] private struct Pt { public int X, Y; }

        private static HashSet<uint> AppPids()
        {
            var name = _app switch { "word" => "WINWORD", "powerpoint" => "POWERPNT", _ => "EXCEL" };
            var ids = new HashSet<uint>();
            foreach (var p in Process.GetProcessesByName(name))
            {
                ids.Add((uint)p.Id);
                p.Dispose();
            }
            return ids;
        }

        private static List<(IntPtr h, string cls, string title)> WindowsOf(HashSet<uint> pids)
        {
            var found = new List<(IntPtr, string, string)>();
            WdEnumWindows((h, _) =>
            {
                WdGetPid(h, out var pid);
                if (!pids.Contains(pid) || !WdIsVisible(h)) return true;
                var c = new StringBuilder(100);
                var t = new StringBuilder(256);
                WdGetClass(h, c, c.Capacity);
                WdGetText(h, t, t.Capacity);
                found.Add((h, c.ToString(), t.ToString()));
                return true;
            }, IntPtr.Zero);
            return found;
        }

        /// <summary>Esc to every Excel window: with one workbook per frame, the
        /// frame that is calculating is not necessarily the active one.</summary>
        private static void PressEscape()
        {
            const uint WmKeyDown = 0x100, WmKeyUp = 0x101;
            foreach (var (h, cls, _) in WindowsOf(AppPids()))
            {
                if (cls != "XLMAIN") continue;
                WdPost(h, WmKeyDown, (IntPtr)0x1B, (IntPtr)0x00010001);
                Thread.Sleep(30);
                WdPost(h, WmKeyUp, (IntPtr)0x1B, unchecked((IntPtr)(long)0xC0010001));
            }
        }

        /// <summary>A box the application is showing and waiting on.</summary>
        private sealed class BlockingDialog
        {
            public IntPtr Hwnd;
            public string Title = "";
            public string Text = "";
            public List<(IUIAutomationElement El, string Name)> Buttons = new();

            public string Key => Hwnd + ":" + Text;

            public bool IsLinksPrompt =>
                Text.Contains("links to one or more external sources", StringComparison.OrdinalIgnoreCase);

            public bool Press(string button)
            {
                foreach (var (el, name) in Buttons)
                {
                    if (!string.Equals(name, button, StringComparison.OrdinalIgnoreCase)) continue;
                    try
                    {
                        el.GetCurrentPattern(UiaInvokePattern, out var p);
                        ((IUIAutomationInvokePattern)p).Invoke();
                        return true;
                    }
                    catch (Exception) { return false; }
                }
                return false;
            }

            public string Describe(string app)
            {
                var name = app switch { "word" => "Word", "powerpoint" => "PowerPoint", _ => "Excel" };
                var text = Text.Length > 0 ? Text : $"a window titled \"{Title}\" that is not a short message";
                var buttons = Buttons.Count > 0 ? string.Join(" / ", Buttons.Select(b => b.Name)) : "no buttons found";
                return $"{name} is waiting for someone to answer a box on screen: \"{text.Replace("\r", " ").Replace("\n", " ").Trim()}\" " +
                       $"(buttons: {buttons}). Nothing can continue until it is answered. " +
                       $"Ask the person at the computer to answer it in {name}, then repeat the request.";
            }
        }

        /// <summary>The visible dialog of this application, if there is one.
        /// Its text and buttons come from UI Automation, asked by window
        /// handle: the box is DirectUI, which shows nothing to the window's
        /// own accessible object, and hit-testing points on the screen read
        /// whatever happened to be on top of it.</summary>
        private static BlockingDialog? FindBlockingDialog()
        {
            foreach (var (h, cls, title) in WindowsOf(AppPids()))
            {
                if (cls is not ("NUIDialog" or "#32770")) continue;
                var box = new BlockingDialog { Hwnd = h, Title = title };
                if (!WdGetRect(h, out var r)) return box;
                // A message box is small. A file dialog is a screenful, with
                // nothing in it a caller needs.
                if (r.R - r.L > 700 || r.B - r.T > 400) return box;
                try
                {
                    var uia = (IUIAutomation)new CUIAutomation();
                    uia.ElementFromHandle(h, out var root);
                    uia.CreateTrueCondition(out var all);
                    root.FindAll(UiaDescendants, all, out var found);
                    found.get_Length(out var n);
                    var texts = new List<string>();
                    for (var i = 0; i < n; i++)
                    {
                        found.GetElement(i, out var el);
                        el.get_CurrentControlType(out var type);
                        el.get_CurrentName(out var name);
                        name ??= "";
                        if (name.Length == 0) continue;
                        if (type == UiaButton)
                        {
                            if (name is not ("Close" or "Context help")) box.Buttons.Add((el, name));
                        }
                        else if (type == UiaText && name != title) texts.Add(name);
                    }
                    box.Text = string.Join(" ", texts);
                }
                catch (Exception e) { Trace($"watchdog: reading the box failed: {e.Message}"); }
                return box;
            }
            return null;
        }

        private const int UiaDescendants = 4, UiaButton = 50000, UiaText = 50020, UiaInvokePattern = 10000;

        // The few UI Automation interfaces this needs, declared by hand so the
        // helper stays free of references (the UIA sidecar uses WPF for the
        // same job, which would double the size of this self-contained file).
        // Members are in the order of the interfaces' vtables; the ones not
        // used are placeholders that keep the later ones in their places.
        [ComImport, Guid("ff48dba4-60ef-4201-aa87-54103eef594e")] private class CUIAutomation { }

        [ComImport, Guid("30cbe57d-d9d0-452a-ab13-7ac5ac4825ee"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IUIAutomation
        {
            void CompareElements(IntPtr a, IntPtr b, out int same);
            void CompareRuntimeIds(IntPtr a, IntPtr b, out int same);
            void GetRootElement(out IUIAutomationElement root);
            void ElementFromHandle(IntPtr hwnd, out IUIAutomationElement element);
            void ElementFromPoint(Pt pt, out IUIAutomationElement element);
            void GetFocusedElement(out IUIAutomationElement element);
            void GetRootElementBuildCache(IntPtr request, out IntPtr element);
            void ElementFromHandleBuildCache(IntPtr hwnd, IntPtr request, out IntPtr element);
            void ElementFromPointBuildCache(Pt pt, IntPtr request, out IntPtr element);
            void GetFocusedElementBuildCache(IntPtr request, out IntPtr element);
            void CreateTreeWalker(IntPtr condition, out IntPtr walker);
            void get_ControlViewWalker(out IntPtr walker);
            void get_ContentViewWalker(out IntPtr walker);
            void get_RawViewWalker(out IntPtr walker);
            void get_RawViewCondition(out IntPtr condition);
            void get_ControlViewCondition(out IntPtr condition);
            void get_ContentViewCondition(out IntPtr condition);
            void CreateCacheRequest(out IntPtr request);
            void CreateTrueCondition(out IUIAutomationCondition condition);
        }

        [ComImport, Guid("352ffba8-0973-437c-a61f-f64cafd81df9"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IUIAutomationCondition { }

        [ComImport, Guid("d22108aa-8ac5-49a5-837b-37bbb3d7591e"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IUIAutomationElement
        {
            void SetFocus();
            void GetRuntimeId(out IntPtr ids);
            void FindFirst(int scope, IUIAutomationCondition condition, out IUIAutomationElement found);
            void FindAll(int scope, IUIAutomationCondition condition, out IUIAutomationElementArray found);
            void FindFirstBuildCache(int scope, IntPtr condition, IntPtr request, out IntPtr found);
            void FindAllBuildCache(int scope, IntPtr condition, IntPtr request, out IntPtr found);
            void BuildUpdatedCache(IntPtr request, out IntPtr updated);
            void GetCurrentPropertyValue(int id, [MarshalAs(UnmanagedType.Struct)] out object value);
            void GetCurrentPropertyValueEx(int id, int ignoreDefault, [MarshalAs(UnmanagedType.Struct)] out object value);
            void GetCachedPropertyValue(int id, [MarshalAs(UnmanagedType.Struct)] out object value);
            void GetCachedPropertyValueEx(int id, int ignoreDefault, [MarshalAs(UnmanagedType.Struct)] out object value);
            void GetCurrentPatternAs(int id, ref Guid riid, out IntPtr pattern);
            void GetCachedPatternAs(int id, ref Guid riid, out IntPtr pattern);
            void GetCurrentPattern(int id, [MarshalAs(UnmanagedType.IUnknown)] out object pattern);
            void GetCachedPattern(int id, [MarshalAs(UnmanagedType.IUnknown)] out object pattern);
            void GetCachedParent(out IntPtr parent);
            void GetCachedChildren(out IntPtr children);
            void get_CurrentProcessId(out int pid);
            void get_CurrentControlType(out int type);
            void get_CurrentLocalizedControlType([MarshalAs(UnmanagedType.BStr)] out string type);
            void get_CurrentName([MarshalAs(UnmanagedType.BStr)] out string name);
        }

        [ComImport, Guid("14314595-b4bc-4055-95f2-58f2e42c9855"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IUIAutomationElementArray
        {
            void get_Length(out int length);
            void GetElement(int index, out IUIAutomationElement element);
        }

        [ComImport, Guid("fb377fbe-8ea6-46d5-9c73-6499642d3059"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IUIAutomationInvokePattern
        {
            void Invoke();
        }
    }
}
