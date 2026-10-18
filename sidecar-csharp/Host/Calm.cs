// An Office window that appears must not raise a hidden taskbar.
//
// This helper is not the program a person is typing in, so when it makes
// Word or PowerPoint visible, Windows will not let that application take the
// foreground and flashes its taskbar button instead. A taskbar set to hide
// itself slides up and stays up until the window is used. Excel was kept out of
// it by showing its frame only once a workbook is in it, without activation
// (WithNewFramesHidden); Word and PowerPoint were made visible at startup, the
// moment the helper began, seconds before any document existed.
//
// Two things, both quiet: the application's hidden frame is shown with
// SW_SHOWNOACTIVATE before `Visible` is set, so there is nothing left for
// `Visible` to activate, and any flash of its frame is stopped, many times a
// second, for the first seconds. A frame that is not flashing is untouched.

using System;
using System.Runtime.InteropServices;
using System.Threading;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        [StructLayout(LayoutKind.Sequential)]
        private struct FlashInfo
        {
            public uint Size;
            public IntPtr Hwnd;
            public uint Flags; // 0 is FLASHW_STOP
            public uint Count;
            public uint Timeout;
        }

        [DllImport("user32.dll")] private static extern bool FlashWindowEx(ref FlashInfo info);

        /// <summary>The window class of this application's main frame.</summary>
        private static string FrameClass() => _app switch
        {
            "word" => "OpusApp",
            "powerpoint" => "PPTFrameClass",
            _ => "XLMAIN",
        };

        /// <summary>Show the application's frame, if it exists and is hidden,
        /// without activating it (SW_SHOWNOACTIVATE). Looked for for up to a
        /// second, since the frame is made a moment after the application.</summary>
        private static void ShowFrameQuietly()
        {
            var cls = FrameClass();
            for (var i = 0; i < 10; i++)
            {
                var shown = false;
                EnumWindows((h, _) =>
                {
                    if (WindowClass(h) == cls && !IsWindowVisible(h)) { ShowWindow(h, 4); shown = true; }
                    return true;
                }, IntPtr.Zero);
                if (shown) return;
                Thread.Sleep(100);
            }
        }

        /// <summary>Stop any flashing of this application's frames for the
        /// next <paramref name="seconds"/> seconds, on a thread of its own.</summary>
        private static void CalmFrames(int seconds)
        {
            var cls = FrameClass();
            var t = new Thread(() =>
            {
                var end = DateTime.UtcNow.AddSeconds(seconds);
                while (DateTime.UtcNow < end)
                {
                    try
                    {
                        EnumWindows((h, _) =>
                        {
                            if (WindowClass(h) == cls)
                            {
                                var info = new FlashInfo { Size = (uint)Marshal.SizeOf<FlashInfo>(), Hwnd = h };
                                FlashWindowEx(ref info);
                            }
                            return true;
                        }, IntPtr.Zero);
                    }
                    catch (Exception) { }
                    Thread.Sleep(30);
                }
            }) { IsBackground = true, Name = "calm-frames" };
            t.Start();
        }
    }
}
