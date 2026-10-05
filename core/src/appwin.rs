//! Showing the console as an application window, on Windows.
//!
//! The console is a web page served from this machine. Opened as a tab in
//! the person's default browser it read as "this needs Chrome" to a tester
//! whose default was Chrome, and nothing about it looked like an
//! application. So it gets a window of its own, in order of preference:
//!   1. `syn-window.exe`, a small program of Syn's own that holds the web
//!      component Windows ships (WebView2): an icon, a title bar, a taskbar
//!      entry and the page, with no browser to be seen;
//!   2. Edge (or Chrome) as an application window (`--app=`: no tabs, no
//!      address bar), when that program is missing or this machine has no
//!      WebView2;
//!   3. the default browser, as before, when neither is there.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

/// What to do before Syn ends because its window was closed.
static BEFORE_EXIT: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Register what the console does between "the window is gone" and the end of
/// the process. Syn used to end on the spot, and the job object that ties its
/// children to it killed the CLI and the browser the CLI had started in the
/// same instant, so the browser never wrote out what it was holding: a site
/// signed in to in the last half minute was signed out at the next start
/// (measured: a cookie set seconds before the window closed was lost every
/// time, and kept after a clean `quit`).
pub fn before_exit(f: impl Fn() + Send + Sync + 'static) {
    let _ = BEFORE_EXIT.set(Box::new(f));
}

/// The window is gone, and so is the reason to run.
#[cfg(windows)]
fn leave() -> ! {
    if let Some(f) = BEFORE_EXIT.get() {
        f();
    }
    std::process::exit(0)
}

#[cfg(any(windows, test))]
use std::path::PathBuf;

/// Where a browser that can show a page as an application window might be,
/// best first. Edge comes first because Windows ships it: a machine whose
/// default browser is anything else, or none, still gets the window, and the
/// person needs no browser of their own for Syn.
#[cfg(any(windows, test))]
pub fn browser_candidates(env: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(d) = env(var) {
            out.push(PathBuf::from(&d).join(r"Microsoft\Edge\Application\msedge.exe"));
        }
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(d) = env(var) {
            out.push(PathBuf::from(&d).join(r"Google\Chrome\Application\chrome.exe"));
        }
    }
    out
}

/// The flags that make Edge or Chrome show `url` as a window of its own. A
/// profile of Syn's own keeps it a separate browser process, so it neither
/// folds into whatever the person has open nor hands the page to it and
/// exits, and so its window closing means something.
#[cfg(any(windows, test))]
pub fn window_args(url: &str, profile: &Path) -> Vec<String> {
    vec![
        format!("--app={url}"),
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--window-size=1280,860".into(),
    ]
}

/// The browser an application window would be shown in, if this machine has
/// one.
#[cfg(windows)]
pub fn app_browser() -> Option<PathBuf> {
    browser_candidates(|k| std::env::var(k).ok()).into_iter().find(|p| p.is_file())
}

/// Whether `open` can show a window here.
pub fn can_open() -> bool {
    #[cfg(windows)]
    {
        host_exe().is_some() || app_browser().is_some()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// How a window of ours is told from any other: the page never changes its
/// title, and an application window shows the page's title and nothing else
/// (a tab in a browser says "Syn - Microsoft Edge"), so this is only ever the
/// window `open` made: `syn-window`'s own (a Windows Forms window) or a
/// browser's app window.
#[cfg(any(windows, test))]
fn is_syn_window(class: &str, title: &str) -> bool {
    (class == "Chrome_WidgetWin_1" || class.starts_with("WindowsForms10.Window.")) && title == "Syn"
}

/// The exit code `syn-window` gives when this machine has no WebView2.
#[cfg(any(windows, test))]
const HOST_NO_RUNTIME: i32 = 3;

/// Whether the window program did not give a window, so the page should be
/// shown another way: no WebView2 on this machine, or it died as it started.
/// One that was open for a while and then went is a window that was closed.
#[cfg(any(windows, test))]
fn host_gave_no_window(code: Option<i32>, lived: Duration) -> bool {
    code == Some(HOST_NO_RUNTIME) || (lived < Duration::from_secs(5) && code != Some(0))
}

#[cfg(windows)]
mod win {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(cb: unsafe extern "system" fn(isize, isize) -> i32, lparam: isize) -> i32;
        fn IsWindowVisible(h: isize) -> i32;
        fn IsIconic(h: isize) -> i32;
        fn ShowWindow(h: isize, cmd: i32) -> i32;
        fn SetForegroundWindow(h: isize) -> i32;
        fn GetWindowTextW(h: isize, buf: *mut u16, max: i32) -> i32;
        fn GetClassNameW(h: isize, buf: *mut u16, max: i32) -> i32;
    }

    fn text(f: unsafe extern "system" fn(isize, *mut u16, i32) -> i32, h: isize) -> String {
        let mut buf = [0u16; 256];
        let n = unsafe { f(h, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    unsafe extern "system" fn collect(h: isize, lparam: isize) -> i32 {
        // SAFETY: `lparam` is the `Vec` `syn_windows` passed, alive for the
        // whole of the EnumWindows call that calls this.
        let found = unsafe { &mut *(lparam as *mut Vec<isize>) };
        if unsafe { IsWindowVisible(h) } != 0 && super::is_syn_window(&text(GetClassNameW, h), &text(GetWindowTextW, h)) {
            found.push(h);
        }
        1
    }

    /// The visible Syn windows, minimized ones included.
    pub fn syn_windows() -> Vec<isize> {
        let mut found: Vec<isize> = Vec::new();
        unsafe { EnumWindows(collect, &mut found as *mut Vec<isize> as isize) };
        found
    }

    /// Bring the first one to the front, from the taskbar if it is there.
    pub fn raise(h: isize) {
        const SW_RESTORE: i32 = 9;
        unsafe {
            if IsIconic(h) != 0 {
                ShowWindow(h, SW_RESTORE);
            }
            SetForegroundWindow(h);
        }
    }
}

/// Show a Syn window that is already open, if there is one. A second start
/// should bring the window forward, not open another.
pub fn raise_existing() -> bool {
    #[cfg(windows)]
    {
        match win::syn_windows().first() {
            Some(&h) => {
                win::raise(h);
                true
            }
            None => false,
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Show `url` as an application window. With `watch`, Syn ends when its
/// windows are all closed, which is what closing the window of an
/// application does; without it the window is started and left. True if a
/// window was started.
///
/// The windows are watched for, not the browser process: a browser already
/// running on the profile takes the page and the process started here exits
/// at once, and closing the window then ended nothing.
#[cfg(windows)]
pub fn open(url: &str, profile: &Path, watch: bool) -> bool {
    if let Some(host) = host_exe() {
        // Beside the profile, with the chats and keys.
        let data = profile.parent().map(|p| p.join("webview")).unwrap_or_else(|| profile.join("webview"));
        let (u, pr) = (url.to_string(), profile.to_path_buf());
        if open_host(&host, url, &data, watch, move || {
            open_browser_window(&u, &pr, watch);
        }) {
            return true;
        }
    }
    open_browser_window(url, profile, watch)
}

/// The window program, beside this one, if it came with Syn.
#[cfg(windows)]
pub fn host_exe() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.parent()?.join("syn-window.exe");
    p.is_file().then_some(p)
}

/// Start the window program. With `watch`, Syn ends when it does (it is our
/// own process, so its end is a window closed, which is the one place that
/// can be said for certain); and if it never gave a window, `fallback` shows
/// the page another way instead.
#[cfg(windows)]
fn open_host(host: &Path, url: &str, data: &Path, watch: bool, fallback: impl FnOnce() + Send + 'static) -> bool {
    use std::process::Command;
    use std::time::Instant;
    let started = Instant::now();
    let Ok(mut child) = Command::new(host).args(["--url", url, "--data"]).arg(data).spawn() else {
        return false;
    };
    std::thread::spawn(move || {
        let code = child.wait().ok().and_then(|s| s.code());
        if host_gave_no_window(code, started.elapsed()) {
            fallback();
        } else if watch {
            leave();
        }
    });
    true
}

/// A browser's application window for the page, watched for if asked.
#[cfg(windows)]
fn open_browser_window(url: &str, profile: &Path, watch: bool) -> bool {
    use std::process::Command;
    use std::time::Instant;
    let Some(browser) = app_browser() else {
        return false;
    };
    if Command::new(browser).args(window_args(url, profile)).spawn().is_err() {
        return false;
    }
    if watch {
        std::thread::spawn(|| {
            // A browser takes a while to show its first window, longer the
            // first time on a new profile. One that never shows is left
            // running: ending Syn then would end it before anyone saw it.
            let waited = Instant::now();
            while win::syn_windows().is_empty() {
                if waited.elapsed() > Duration::from_secs(90) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            // Gone for two polls running: closed, not just between windows.
            let mut gone = 0;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                gone = if win::syn_windows().is_empty() { gone + 1 } else { 0 };
                if gone >= 2 {
                    leave();
                }
            }
        });
    }
    true
}

#[cfg(not(windows))]
pub fn open(_url: &str, _profile: &Path, _watch: bool) -> bool {
    false
}

/// Whether what answers at `addr` is a Syn console: a second start, with the
/// first still running, should show its window rather than fail to bind the
/// port and vanish. Anything else on that port is not ours to open.
pub fn console_is_syn(addr: &str) -> bool {
    let Some(sock) = addr.to_socket_addrs().ok().and_then(|mut a| a.next()) else {
        return false;
    };
    let Ok(mut s) = TcpStream::connect_timeout(&sock, Duration::from_millis(500)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    // One write, so the request is one segment: a server that answers after
    // reading the first piece and then closes would otherwise reset the
    // connection on the rest, and take the answer with it.
    let req = format!("GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    // The title is near the top of the page.
    let mut head = Vec::new();
    let _ = s.take(16 * 1024).read_to_end(&mut head);
    String::from_utf8_lossy(&head).contains("<title>Syn</title>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn the_app_window_tries_edge_first_and_needs_no_browser_of_the_users() {
        let env = |k: &str| match k {
            "ProgramFiles(x86)" => Some("PF86".to_string()),
            "ProgramFiles" => Some("PF".to_string()),
            "LOCALAPPDATA" => Some("LOCAL".to_string()),
            _ => None,
        };
        let c: Vec<String> = browser_candidates(env).iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let first_chrome = c.iter().position(|p| p.ends_with("chrome.exe")).unwrap();
        let last_edge = c.iter().rposition(|p| p.ends_with("msedge.exe")).unwrap();
        assert!(c[0].starts_with("PF86") && c[0].ends_with("msedge.exe"), "{c:?}");
        assert!(last_edge < first_chrome, "every Edge path before any Chrome path: {c:?}");
        // No Program Files known: nothing to try, so the page goes to the
        // default browser rather than a path guessed from nothing.
        assert!(browser_candidates(|_| None).is_empty());
    }

    #[test]
    fn the_app_window_flags_give_a_page_its_own_window() {
        let profile = Path::new(r"C:\Program Files\Syn\.agent\app-profile");
        let a = window_args("http://127.0.0.1:7777/", profile);
        assert!(a.contains(&"--app=http://127.0.0.1:7777/".to_string()), "{a:?}");
        // One argument for a profile path with spaces in it: Command passes
        // each as it is, so nothing is split at "Program Files".
        assert!(
            a.iter().any(|x| x.starts_with("--user-data-dir=") && x.ends_with("app-profile") && x.contains("Program Files")),
            "{a:?}"
        );
        assert!(a.contains(&"--no-first-run".to_string()));
    }

    #[test]
    fn a_window_program_that_gave_no_window_is_told_from_one_that_was_closed() {
        let long = Duration::from_secs(600);
        let short = Duration::from_secs(1);
        // No WebView2 here, at once or later: show the page another way.
        assert!(host_gave_no_window(Some(HOST_NO_RUNTIME), short));
        assert!(host_gave_no_window(Some(HOST_NO_RUNTIME), long));
        // It died as it started.
        assert!(host_gave_no_window(Some(1), short));
        assert!(host_gave_no_window(None, short));
        // A window that was open and was closed, or fell over mid-use, is not
        // a reason to open another one behind the person's back.
        assert!(!host_gave_no_window(Some(0), short));
        assert!(!host_gave_no_window(Some(0), long));
        assert!(!host_gave_no_window(Some(1), long));
        assert!(!host_gave_no_window(None, long));
    }

    #[test]
    fn only_the_apps_own_window_is_taken_for_a_syn_window() {
        assert!(is_syn_window("Chrome_WidgetWin_1", "Syn"));
        assert!(is_syn_window("WindowsForms10.Window.8.app.0.1f550a4_r3_ad1", "Syn"));
        assert!(!is_syn_window("WindowsForms10.Window.8.app.0.1f550a4_r3_ad1", "Syn - notes"));
        // A tab in a browser carries the browser's name, and another program's
        // window of the same class is not ours.
        assert!(!is_syn_window("Chrome_WidgetWin_1", "Syn - Microsoft Edge"));
        assert!(!is_syn_window("Chrome_WidgetWin_1", "Synology"));
        assert!(!is_syn_window("Notepad", "Syn"));
    }

    /// A server that answers one request with `body`, as a console would.
    fn serve_once(body: &'static str) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                // The whole request, to its blank line, before answering.
                let mut got = Vec::new();
                let mut buf = [0u8; 1024];
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        addr
    }

    #[test]
    fn a_running_console_is_known_by_its_page_and_nothing_else_is_taken_for_one() {
        assert!(console_is_syn(&serve_once("<html><head><title>Syn</title></head></html>")));
        // Something else on the port is not ours to open a window onto.
        assert!(!console_is_syn(&serve_once("<html><head><title>Router admin</title></head></html>")));
        // And nothing listening is not a console either.
        let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
        assert!(!console_is_syn(&free));
    }
}
