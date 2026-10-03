// A cell left in edit mode.
//
// While a cell is being typed in (the status bar reads "Cell Mode Enter",
// "Edit" or "Point"), Excel refuses every automation call: 0x800AC472, or
// RPC_E_CALL_REJECTED until the message filter's 15 s are up. No box is on
// screen to find, so nothing looked wrong, and a 50-message run spent seven
// turns being told "press Esc in Excel" by a model that had nobody to tell:
// it ended each turn with that request and the work stopped.
//
// What a colleague would do depends on whether anyone is there. If a person
// is at the keyboard, the cell is theirs, and the right answer is to say so
// and wait. If nobody has touched the keyboard or mouse for a minute, the
// edit is a stray (a key that went to the wrong window, a click left over)
// and pressing Esc, which throws away only the unfinished edit, is what the
// model was asking the person to do. So: Esc only when the machine has been
// idle for AGENT_OFFICE_IDLE_SECS (60), and the call then runs again.

using System;
using System.Linq;
using System.Runtime.InteropServices;
using System.Threading;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        /// <summary>How long the keyboard and mouse must have been untouched
        /// before a stuck cell edit is taken to be nobody's.</summary>
        private static readonly int IdleNeeded =
            int.TryParse(Environment.GetEnvironmentVariable("AGENT_OFFICE_IDLE_SECS"), out var v) && v >= 0 ? v : 60;

        [StructLayout(LayoutKind.Sequential)] private struct LastInput { public uint Size; public uint Time; }
        [DllImport("user32.dll")] private static extern bool GetLastInputInfo(ref LastInput info);

        private static double IdleSeconds()
        {
            var info = new LastInput { Size = (uint)Marshal.SizeOf<LastInput>() };
            if (!GetLastInputInfo(ref info)) return 0; // unknown: treat the person as present
            return unchecked((uint)Environment.TickCount - info.Time) / 1000.0;
        }

        private static bool IsBusyFailure(string reply) =>
            reply.StartsWith("{\"ok\":false", StringComparison.Ordinal)
            && (reply.Contains("800AC472", StringComparison.OrdinalIgnoreCase)
                || reply.Contains("modal dialog or busy app", StringComparison.Ordinal));

        /// <summary>The mode Excel's status bar shows ("Ready", "Enter",
        /// "Edit", "Point", ...), or "" when it cannot be read. Read through
        /// UI Automation: Excel will not answer the object model in this
        /// state, which is the point.</summary>
        private static string ExcelCellMode()
        {
            var uia = (IUIAutomation)new CUIAutomation();
            uia.CreateTrueCondition(out var all);
            var looked = 0;
            foreach (var (h, cls, _) in WindowsOf(AppPids()))
            {
                if (cls != "XLMAIN" || looked++ >= 4) continue;
                uia.ElementFromHandle(h, out var root);
                root.FindAll(UiaDescendants, all, out var found);
                found.get_Length(out var n);
                for (var i = 0; i < n; i++)
                {
                    found.GetElement(i, out var el);
                    el.get_CurrentName(out var name);
                    if (name != null && name.StartsWith("Cell Mode ", StringComparison.Ordinal))
                        return name["Cell Mode ".Length..].Trim();
                }
            }
            return "";
        }

        /// <summary>Runs on the STA thread, after a call that Excel refused.
        /// Clears a stray cell edit and runs the call again, or says in
        /// words why it could not and what to do.</summary>
        private static string RecoverFromCellEdit(dynamic app, string line, string reply)
        {
            if (_app != "excel" || !IsBusyFailure(reply)) return reply;
            string mode;
            try { mode = ExcelCellMode(); }
            catch (Exception e) { Trace($"cell mode unreadable: {e.Message}"); return reply; }
            if (mode is not ("Enter" or "Edit" or "Point")) return reply;

            var idle = IdleSeconds();
            if (idle < IdleNeeded)
                return Fail($"Excel is in cell {mode} mode: someone is typing in a cell, and Excel takes no calls until that is finished. " +
                            $"Syn did not press Esc because the keyboard or mouse was used {idle:n0} s ago. " +
                            "Finish the edit (Enter) or press Esc in Excel, then repeat the request. Nothing was changed.");

            Console.WriteLine($"excel: stuck in cell {mode} mode with nobody at the keyboard for {idle:n0} s; pressing Esc");
            PressEscape();
            Thread.Sleep(700);
            var again = Serve(app, line);
            if (IsBusyFailure(again))
                return Fail($"Excel was in cell {mode} mode and Syn pressed Esc to leave it, but Excel still takes no calls. " +
                            "Ask the person at the computer to look at Excel (a box or an edit may be open), then repeat the request.");
            if (!IsOk(again)) return again;
            var note = $" (Excel had been left in cell {mode} mode with nobody at the keyboard; Syn pressed Esc to leave it, and the call then ran.)";
            return again.Substring(0, again.Length - 2) + Esc(note) + "\"}";
        }
    }
}
