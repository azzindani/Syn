// A macro that fails must not hang the run.
//
// Live, the first macro that divided by zero put up VBA's "Run-time error
// '11': Division by zero" box (Continue / End / Debug / Help) in front of
// the user, and Application.Run waited on it. The message filter cannot
// cancel that: the call is not rejected, it is simply not finished. The run
// sat there for minutes, and when someone clicked End what came back was
// "0x800A9C68", which teaches a model nothing. A compile error does the same
// with an OK box, and leaves the VBA editor open on top.
//
// So while a macro runs, a watcher looks for VBA's own dialog in that Excel,
// reads what it says, and presses End (or OK) as a person would. The words
// in the box are the error the model gets back.

using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        private delegate bool EnumProc(IntPtr hwnd, IntPtr lParam);

        [DllImport("user32.dll")] private static extern bool EnumWindows(EnumProc f, IntPtr l);
        [DllImport("user32.dll")] private static extern bool EnumChildWindows(IntPtr parent, EnumProc f, IntPtr l);
        [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
        [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr hwnd);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr hwnd, StringBuilder s, int n);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetClassName(IntPtr hwnd, StringBuilder s, int n);
        [DllImport("user32.dll")] private static extern bool PostMessage(IntPtr hwnd, uint msg, IntPtr w, IntPtr l);

        private const uint BmClick = 0x00F5;

        private static string WindowText(IntPtr h)
        {
            var s = new StringBuilder(2048);
            GetWindowText(h, s, s.Capacity);
            return s.ToString();
        }

        private static string WindowClass(IntPtr h)
        {
            var s = new StringBuilder(256);
            GetClassName(h, s, s.Capacity);
            return s.ToString();
        }

        /// <summary>VBA's error box in that process, read and dismissed, or
        /// null when there is none. End for a run-time error (Debug would
        /// open the editor in break mode), OK for a compile error.</summary>
        private static string? DismissVbaDialog(uint pid)
        {
            string? said = null;
            EnumWindows((h, _) =>
            {
                GetWindowThreadProcessId(h, out var p);
                if (p != pid || !IsWindowVisible(h) || WindowClass(h) != "#32770") return true;
                if (!WindowText(h).StartsWith("Microsoft Visual Basic", StringComparison.Ordinal)) return true;
                var text = new List<string>();
                IntPtr end = IntPtr.Zero, ok = IntPtr.Zero;
                EnumChildWindows(h, (k, _) =>
                {
                    var t = WindowText(k);
                    var cls = WindowClass(k);
                    if (cls == "Static" && t.Trim() != "") text.Add(t.Trim());
                    if (cls == "Button" && t == "&End") end = k;
                    if (cls == "Button" && (t == "OK" || t == "&OK")) ok = k;
                    return true;
                }, IntPtr.Zero);
                var press = end != IntPtr.Zero ? end : ok;
                if (press == IntPtr.Zero) return true;
                PostMessage(press, BmClick, IntPtr.Zero, IntPtr.Zero);
                said = string.Join(" ", text).Replace("\r", " ").Replace("\n", " ");
                while (said.Contains("  ")) said = said.Replace("  ", " ");
                return false;
            }, IntPtr.Zero);
            return said;
        }

        /// <summary>Run > Reset in that Excel's VBA editor, from the watcher's
        /// own thread: the helper's thread is the one waiting on the
        /// paused macro. Needs VBA project access; without it, nothing.</summary>
        private static void ResetVba(uint pid)
        {
            try
            {
                CLSIDFromProgID("Excel.Application", out var clsid);
                GetActiveObject(ref clsid, IntPtr.Zero, out var o);
                dynamic xl = o;
                GetWindowThreadProcessId(new IntPtr((int)xl.Hwnd), out var p);
                if (p != pid) return;
                object? found = xl.VBE.CommandBars.FindControl(1, 228); // Run > Reset
                if (found is null) return;
                dynamic reset = found;
                if ((bool)reset.Enabled) reset.Execute();
            }
            catch { }
        }

        /// <summary>Run `call` with a watcher on that Excel's VBA dialogs;
        /// the text of the first one it dismissed, if any.</summary>
        private static string? WithVbaWatch(object appO, Action call)
        {
            dynamic app = appO;
            uint pid = 0;
            try { GetWindowThreadProcessId(new IntPtr((int)app.Hwnd), out pid); } catch { }
            if (pid == 0) { call(); return null; }
            string? first = null;
            var done = false;
            var watcher = new Thread(() =>
            {
                while (!Volatile.Read(ref done))
                {
                    var said = DismissVbaDialog(pid);
                    if (said != null && first == null) first = said;
                    // Closing a compile error met mid-run leaves VBA paused
                    // in the debugger, and the call waits on that too.
                    if (said != null && said.StartsWith("Compile error", StringComparison.Ordinal))
                    {
                        Thread.Sleep(300);
                        ResetVba(pid);
                    }
                    Thread.Sleep(150);
                }
            }) { IsBackground = true, Name = "vba-watch" };
            watcher.Start();
            try { call(); }
            finally
            {
                Volatile.Write(ref done, true);
                watcher.Join(1000);
            }
            if (first != null)
            {
                // A compile error leaves the editor open on the bad line.
                try { app.VBE.MainWindow.Visible = false; } catch { }
            }
            return first;
        }
    }
}
