// syn-window: Syn's own window.
//
// The console is a page (widget/index.html, served by ui.exe on this machine).
// This is the window that shows it, as a program of its own: an icon, a title
// bar, a taskbar button, and no tabs or address bar. It holds Microsoft's
// WebView2, the web component Windows ships, and nothing else: no logic of
// Syn's lives here, so what runs in it is exactly the page the console serves.
//
//   syn-window.exe --url http://127.0.0.1:7777/ [--data <folder for the page's storage>]
//
// Exit codes: 0 when the window is closed; 3 when this machine has no
// WebView2 runtime, which tells ui.exe to show the page another way.

using System;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Windows.Forms;
using Microsoft.Web.WebView2.Core;
using Microsoft.Web.WebView2.WinForms;

namespace SynWindow
{
    internal static class Program
    {
        /// The code for "no WebView2 here": the caller falls back to a browser's app window.
        internal const int NoRuntime = 3;

        private static string? Arg(string[] args, string name)
        {
            var i = Array.IndexOf(args, name);
            return i >= 0 && i + 1 < args.Length ? args[i + 1] : null;
        }

        [STAThread]
        private static int Main(string[] args)
        {
            var url = Arg(args, "--url") ?? "http://127.0.0.1:7777/";
            var data = Arg(args, "--data") ?? Path.Combine(AppContext.BaseDirectory, ".agent", "webview");

            // Only ever the console on this machine.
            if (!Uri.TryCreate(url, UriKind.Absolute, out var uri) || uri.Host != "127.0.0.1")
            {
                Console.Error.WriteLine("syn-window: --url must be an address on 127.0.0.1");
                return 2;
            }

            string? version = null;
            try { version = CoreWebView2Environment.GetAvailableBrowserVersionString(); }
            catch { /* the same as none */ }
            if (string.IsNullOrEmpty(version)) return NoRuntime;

            Application.SetHighDpiMode(HighDpiMode.PerMonitorV2);
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);
            Application.Run(new Shell(uri, data));
            return Shell.Failed ? NoRuntime : 0;
        }
    }

    internal sealed class Shell : Form
    {
        internal static bool Failed;

        private readonly WebView2 _view = new WebView2();
        private readonly Uri _home;
        private readonly string _data;

        internal Shell(Uri home, string data)
        {
            _home = home;
            _data = data;
            Text = "Syn";
            Icon = Icon.ExtractAssociatedIcon(Application.ExecutablePath);
            AutoScaleMode = AutoScaleMode.Dpi;
            MinimumSize = new Size(720, 480);
            ClientSize = new Size(1280, 860);
            StartPosition = FormStartPosition.CenterScreen;
            _view.Dock = DockStyle.Fill;
            // Under the page while it loads: the window's own colour, not a white flash.
            _view.DefaultBackgroundColor = SystemColors.Window;
            Controls.Add(_view);
            Restore();
            Load += async (_, _) => await Start();
            FormClosing += (_, _) => Save();
        }

        private async System.Threading.Tasks.Task Start()
        {
            try
            {
                Directory.CreateDirectory(_data);
                var env = await CoreWebView2Environment.CreateAsync(null, _data);
                await _view.EnsureCoreWebView2Async(env);
            }
            catch
            {
                // No usable runtime: say so by the exit code, and let the caller show the page another way.
                Failed = true;
                Close();
                return;
            }
            var web = _view.CoreWebView2;
            var s = web.Settings;
            // A window of an application, not of a browser: nothing to type an address into.
            s.AreDevToolsEnabled = Environment.GetEnvironmentVariable("SYN_DEVTOOLS") == "1";
            s.IsStatusBarEnabled = false;
            s.IsPasswordAutosaveEnabled = false;
            s.IsGeneralAutofillEnabled = false;
            s.IsSwipeNavigationEnabled = false;

            // The page is the console and only the console. A link to anywhere
            // else opens in the person's own browser, not in this window.
            web.NavigationStarting += (_, e) =>
            {
                if (Uri.TryCreate(e.Uri, UriKind.Absolute, out var u) && u.Host != _home.Host && u.Scheme != "about" && u.Scheme != "data")
                {
                    e.Cancel = true;
                    Open(u);
                }
            };
            web.NewWindowRequested += (_, e) =>
            {
                e.Handled = true;
                if (Uri.TryCreate(e.Uri, UriKind.Absolute, out var u)) Open(u);
            };
            // A page process that dies is brought back, not left blank.
            web.ProcessFailed += (_, e) =>
            {
                if (e.ProcessFailedKind == CoreWebView2ProcessFailedKind.BrowserProcessExited) { Failed = true; Close(); }
                else _view.Reload();
            };
            web.Navigate(_home.ToString());
        }

        private static void Open(Uri u)
        {
            if (u.Scheme != Uri.UriSchemeHttp && u.Scheme != Uri.UriSchemeHttps) return;
            try { Process.Start(new ProcessStartInfo(u.ToString()) { UseShellExecute = true }); }
            catch { /* nothing to open it with: nothing to do */ }
        }

        // Where the window was, so it comes back there.
        private string StateFile => Path.Combine(Path.GetDirectoryName(_data.TrimEnd('\\', '/')) ?? _data, "window.txt");

        private void Save()
        {
            try
            {
                var b = WindowState == FormWindowState.Normal ? Bounds : RestoreBounds;
                File.WriteAllText(StateFile, $"{b.X},{b.Y},{b.Width},{b.Height},{(WindowState == FormWindowState.Maximized ? 1 : 0)}");
            }
            catch { /* a window that cannot remember is still a window */ }
        }

        private void Restore()
        {
            try
            {
                var p = File.ReadAllText(StateFile).Split(',');
                if (p.Length != 5) return;
                var r = new Rectangle(int.Parse(p[0]), int.Parse(p[1]), int.Parse(p[2]), int.Parse(p[3]));
                // Only if some of it is on a screen that is still there.
                var onScreen = false;
                foreach (var sc in Screen.AllScreens)
                    if (sc.WorkingArea.IntersectsWith(new Rectangle(r.X, r.Y, Math.Min(r.Width, 200), Math.Min(r.Height, 100)))) onScreen = true;
                if (!onScreen || r.Width < 720 || r.Height < 480) return;
                StartPosition = FormStartPosition.Manual;
                Bounds = r;
                if (p[4] == "1") WindowState = FormWindowState.Maximized;
            }
            catch { /* first run, or a file that is not ours */ }
        }
    }
}
