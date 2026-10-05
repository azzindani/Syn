//! Syn's own browser: found on this machine, started on a profile of its
//! own, and stopped with Syn.
//!
//! The browser hand used to attach to a Chrome the person had started
//! themselves with `--remote-debugging-port` and a `--user-data-dir`, and
//! told it where in `.env`. Nobody installing an application does that, and
//! since Chrome 136 a debugging port is ignored on the everyday profile
//! anyway, so "drive my own Chrome" was never going to work as it came. So
//! Syn starts a Chrome or Edge of its own, on its own profile (a folder
//! under `.agent`, which keeps a sign-in between runs), on a port the system
//! picks, and a person signs in to a site in that window once.
//!
//! Three rules follow from the browser being Syn's and not the person's:
//!   - it is started when a page is first wanted, never at startup;
//!   - it listens on loopback only, on a port nobody chose (no fixed 9222 for
//!     another program to find, and none that can already be taken);
//!   - it goes when Syn goes, by the job object that stops every child, and
//!     by a polite `Browser.close` first so the profile is saved cleanly.
//!
//! A browser the person points Syn at with `AGENT_CDP` is the other case and
//! is never started or closed here.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// `off` turns the browser away entirely; a path names the executable to use.
/// Unset, Syn looks for Chrome and then Edge.
pub const BROWSER_ENV: &str = "AGENT_BROWSER";
/// `1` runs the browser with no window, for a server or a test.
pub const HEADLESS_ENV: &str = "AGENT_BROWSER_HEADLESS";

/// What `AGENT_BROWSER` says, as a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setting {
    Auto,
    Off,
    Path(PathBuf),
}

pub fn setting(env: impl Fn(&str) -> Option<String>) -> Setting {
    match env(BROWSER_ENV).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
        None => Setting::Auto,
        Some(v) if v.eq_ignore_ascii_case("off") || v.eq_ignore_ascii_case("none") || v == "0" => Setting::Off,
        Some(v) if v.eq_ignore_ascii_case("auto") => Setting::Auto,
        Some(v) => Setting::Path(PathBuf::from(v)),
    }
}

/// Where a Chromium-based browser might be, best first. Chrome ahead of Edge
/// because it is what most people sign in to their accounts with; Edge is
/// there on every Windows machine, which is what makes this work on a bare
/// one.
pub fn candidates(env: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    // The sandbox and CI name theirs.
    for var in ["PW_CHROMIUM", "SYN_BROWSER"] {
        if let Some(p) = env(var).filter(|p| !p.trim().is_empty()) {
            out.push(PathBuf::from(p));
        }
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(d) = env(var) {
            out.push(PathBuf::from(&d).join(r"Google\Chrome\Application\chrome.exe"));
        }
    }
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(d) = env(var) {
            out.push(PathBuf::from(&d).join(r"Microsoft\Edge\Application\msedge.exe"));
        }
    }
    for p in [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/opt/pw-browsers/chromium",
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/microsoft-edge",
    ] {
        out.push(PathBuf::from(p));
    }
    out
}

/// The browser Syn would start, or why there is none.
pub fn find() -> Result<PathBuf, String> {
    let env = |k: &str| std::env::var(k).ok();
    match setting(env) {
        Setting::Off => Err("the browser is turned off (AGENT_BROWSER=off)".into()),
        Setting::Path(p) if p.is_file() => Ok(p),
        Setting::Path(p) => Err(format!("AGENT_BROWSER names {}, which is not a file", p.display())),
        Setting::Auto => candidates(env)
            .into_iter()
            .find(|p| p.is_file())
            .ok_or_else(|| "no Chrome or Edge was found on this computer (set AGENT_BROWSER to the browser's program)".into()),
    }
}

/// Whether Syn could start a browser here, without starting one.
pub fn available() -> bool {
    find().is_ok()
}

pub fn headless() -> bool {
    std::env::var(HEADLESS_ENV).is_ok_and(|v| v.trim() == "1")
}

/// The folder the browser keeps its profile in. Beside the chats and the
/// keys, so one `.agent` holds everything Syn keeps, and a person who wants
/// the browser signed out deletes one folder.
pub fn profile_dir() -> PathBuf {
    crate::chats::home().join("browser-profile")
}

/// The flags that make a browser Syn's to drive.
pub fn args(profile: &Path, headless: bool) -> Vec<String> {
    let mut a = vec![
        // Chosen by the system and written to `DevToolsActivePort`: no fixed
        // number for anything else to find, and none already in use.
        "--remote-debugging-port=0".to_string(),
        "--remote-debugging-address=127.0.0.1".to_string(),
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        // A first-run choice dialog would sit in front of every page.
        "--disable-search-engine-choice-screen".into(),
        // Not "restore your pages?" after a stop that was not a clean quit.
        "--hide-crash-restore-bubble".into(),
        "--window-size=1280,900".into(),
    ];
    if headless {
        a.push("--headless=new".into());
        a.push("--disable-gpu".into());
        // A container or a CI runner without a sandbox to offer.
        if std::env::var("SYN_BROWSER_NO_SANDBOX").is_ok_and(|v| v == "1") || cfg!(target_os = "linux") && is_root() {
            a.push("--no-sandbox".into());
        }
    }
    a.push("about:blank".into());
    a
}

#[cfg(unix)]
fn is_root() -> bool {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: a call that reads the process's own identity.
    unsafe { geteuid() == 0 }
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

/// `DevToolsActivePort`: the port on the first line, the browser's socket
/// path on the second. Written once the browser is listening, and left
/// behind by one that died.
pub fn read_port_file(text: &str) -> Option<String> {
    let port: u16 = text.lines().next()?.trim().parse().ok().filter(|p| *p != 0)?;
    Some(format!("127.0.0.1:{port}"))
}

/// Whether something answers as a DevTools endpoint at `addr`. A short
/// knock first: on Windows a connect to a closed local port is retried for
/// two seconds before it fails, and this is asked before every turn.
pub fn alive(addr: &str) -> bool {
    use std::net::ToSocketAddrs;
    let Some(sock) = addr.to_socket_addrs().ok().and_then(|mut a| a.next()) else { return false };
    std::net::TcpStream::connect_timeout(&sock, Duration::from_millis(150)).is_ok()
}

/// The address of a browser already running on Syn's profile: one a past
/// session left (Syn ending by a kill leaves nothing, the job object sees to
/// that, but a browser can outlive a crashed helper) or one still starting.
pub fn running_on(profile: &Path) -> Option<String> {
    let addr = read_port_file(&std::fs::read_to_string(profile.join("DevToolsActivePort")).ok()?)?;
    alive(&addr).then_some(addr)
}

/// A browser Syn started: the process and where it listens.
#[derive(Debug)]
pub struct Started {
    pub child: Option<Child>,
    pub addr: String,
}

/// Start the browser on `profile` and wait until it listens.
///
/// A profile that is already in use by a running browser hands the new
/// process to the old one and exits at once, ignoring the port flag: so when
/// the child exits, the port file is read once more before giving up, and a
/// browser that is listening is reused instead of reported as a failure.
pub fn start(exe: &Path, profile: &Path, headless: bool) -> Result<Started, String> {
    std::fs::create_dir_all(profile).map_err(|e| format!("cannot make the browser's profile folder {}: {e}", profile.display()))?;
    if let Some(addr) = running_on(profile) {
        return Ok(Started { child: None, addr });
    }
    // A file left by a browser that is gone would be read as this one's.
    let _ = std::fs::remove_file(profile.join("DevToolsActivePort"));

    let mut cmd = Command::new(exe);
    cmd.args(args(profile, headless)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    crate::desk::no_inherit::prepare(&mut cmd);
    // Dies with this process however it ends; `Cdp`'s drop is the polite
    // version of the same, for the clean exit.
    crate::tether::bind(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", exe.display()))?;
    crate::tether::adopt(&child);

    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if let Some(addr) = running_on(profile) {
            return Ok(Started { child: Some(child), addr });
        }
        if let Ok(Some(status)) = child.try_wait() {
            // Handed to a browser that was already there? Give it a moment
            // to write its port.
            let settle = Instant::now() + Duration::from_secs(3);
            while Instant::now() < settle {
                if let Some(addr) = running_on(profile) {
                    return Ok(Started { child: None, addr });
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            return Err(format!(
                "{} exited ({status}) without opening a debugging port. If a window of Syn's browser is already open without one, close it and try again.",
                exe.display()
            ));
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err(format!("{} did not open its debugging port within 45 s", exe.display()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn the_setting_reads_off_a_path_or_nothing() {
        assert_eq!(setting(env(&[])), Setting::Auto);
        assert_eq!(setting(env(&[("AGENT_BROWSER", "  ")])), Setting::Auto);
        assert_eq!(setting(env(&[("AGENT_BROWSER", "off")])), Setting::Off);
        assert_eq!(setting(env(&[("AGENT_BROWSER", "OFF")])), Setting::Off);
        assert_eq!(setting(env(&[("AGENT_BROWSER", "auto")])), Setting::Auto);
        assert_eq!(setting(env(&[("AGENT_BROWSER", r"D:\b\chrome.exe")])), Setting::Path(PathBuf::from(r"D:\b\chrome.exe")));
    }

    #[test]
    fn chrome_is_tried_before_edge_and_both_are_found_without_a_path() {
        let c = candidates(env(&[("ProgramFiles", r"C:\Program Files"), ("ProgramFiles(x86)", r"C:\Program Files (x86)")]));
        let at = |needle: &str| c.iter().position(|p| p.to_string_lossy().contains(needle)).unwrap_or(usize::MAX);
        assert!(at("chrome.exe") < at("msedge.exe"), "{c:?}");
        assert!(at("msedge.exe") < usize::MAX, "Edge ships with Windows and must be a candidate: {c:?}");
        // The sandbox names its own and that comes first.
        let c = candidates(env(&[("PW_CHROMIUM", "/opt/x/chromium")]));
        assert_eq!(c[0], PathBuf::from("/opt/x/chromium"));
    }

    #[test]
    fn the_browser_is_started_on_a_port_nobody_chose_and_on_loopback() {
        let a = args(Path::new("/p/browser-profile"), false);
        assert!(a.contains(&"--remote-debugging-port=0".to_string()), "{a:?}");
        assert!(a.contains(&"--remote-debugging-address=127.0.0.1".to_string()), "{a:?}");
        assert!(a.iter().any(|x| x.starts_with("--user-data-dir=") && x.contains("browser-profile")), "{a:?}");
        assert!(!a.iter().any(|x| x.contains("9222")), "no fixed port for another program to find: {a:?}");
        assert!(!a.iter().any(|x| x.starts_with("--headless")), "{a:?}");
        assert!(args(Path::new("/p"), true).iter().any(|x| x == "--headless=new"));
        assert_eq!(a.last().unwrap(), "about:blank", "the page to start on goes last");
    }

    #[test]
    fn the_port_file_is_read_as_the_port_on_its_first_line() {
        assert_eq!(read_port_file("43521\n/devtools/browser/8f2a-1b\n").as_deref(), Some("127.0.0.1:43521"));
        assert_eq!(read_port_file("43521").as_deref(), Some("127.0.0.1:43521"));
        // Not a port: refuse rather than connect to nowhere.
        assert_eq!(read_port_file(""), None);
        assert_eq!(read_port_file("0\n/x"), None);
        assert_eq!(read_port_file("soon\n"), None);
        assert_eq!(read_port_file("99999\n"), None);
    }

    #[test]
    fn a_port_file_nothing_answers_on_is_not_a_browser() {
        // A browser that died leaves its file behind. Reading it must not
        // be mistaken for one that is running.
        let dir = std::env::temp_dir().join(format!("syn-browser-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A port that was free a moment ago and has nothing on it.
        let free = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        std::fs::write(dir.join("DevToolsActivePort"), format!("{free}\n/devtools/browser/x\n")).unwrap();
        assert_eq!(running_on(&dir), None);
        // And one that is listening is.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::fs::write(dir.join("DevToolsActivePort"), format!("{port}\n/devtools/browser/x\n")).unwrap();
        assert_eq!(running_on(&dir), Some(format!("127.0.0.1:{port}")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_headless_browser_starts_on_its_own_profile_and_a_second_start_reuses_it() {
        // Needs a Chromium; where there is none the claim is made by the
        // live tests that CI requires one for. Fast: no page is loaded.
        let Ok(exe) = find() else {
            eprintln!("skipped: no browser here");
            return;
        };
        let dir = std::env::temp_dir().join(format!("syn-browser-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut first = start(&exe, &dir, true).expect("the browser starts");
        assert!(first.child.is_some(), "a first start owns the process");
        assert!(first.addr.starts_with("127.0.0.1:"), "{}", first.addr);
        assert!(alive(&first.addr));
        // The same profile, asked again: the running browser, not a second
        // process fighting it for the profile.
        let second = start(&exe, &dir, true).expect("a second start finds the first");
        assert!(second.child.is_none(), "a reused browser is not owned");
        assert_eq!(second.addr, first.addr);
        let child = first.child.as_mut().unwrap();
        let _ = child.kill();
        let _ = child.wait();
        // Chrome's own children outlive the launcher briefly; the profile
        // folder cannot be removed while they hold it. Not worth waiting on.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_executable_that_is_not_there_is_refused_with_its_name() {
        let dir = std::env::temp_dir().join(format!("syn-browser-test2-{}", std::process::id()));
        let e = start(Path::new("/no/such/browser"), &dir, true).unwrap_err();
        assert!(e.contains("could not start") && e.contains("no/such/browser"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
