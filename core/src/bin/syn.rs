//! syn: what the Start menu and the desktop shortcut run.
//!
//! The console (`ui`) is a console program, so starting it from a shortcut
//! put a terminal window on the taskbar beside the application window, and
//! closing that terminal stopped Syn. This is the same start without one:
//! a Windows program with no console of its own, which starts `ui` with no
//! window at all and leaves the Syn window as the only thing on the screen.
//! Closing that window stops Syn (`ui --open` watches it).
//!
//! Three things it does that a plain shortcut to `ui` would not:
//!   * Syn already running (a second double-click): bring its window to the
//!     front, rather than let the second start fail to bind the port and
//!     vanish.
//!   * No browser that can show an application window: keep `ui`'s console
//!     window, because it is then the only way to stop Syn.
//!   * `ui` failing as it starts: say so in a box. A program with no console
//!     has nowhere else to say anything.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const ADDR: &str = "127.0.0.1:7777";
const URL: &str = "http://127.0.0.1:7777/";

#[cfg(windows)]
mod sys {
    use std::os::windows::process::CommandExt;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }

    /// The process gets a console that nothing can see, which its own
    /// children inherit: no window for `ui`, its CLI, `curl` or a helper.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    /// A console window of its own, which is what stops Syn when closed.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    pub fn hide_console(cmd: &mut std::process::Command, hide: bool) {
        cmd.creation_flags(if hide { CREATE_NO_WINDOW } else { CREATE_NEW_CONSOLE });
    }

    pub fn alert(text: &str) {
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        // MB_OK | MB_ICONERROR
        unsafe { MessageBoxW(0, wide(text).as_ptr(), wide("Syn").as_ptr(), 0x10) };
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn hide_console(_cmd: &mut std::process::Command, _hide: bool) {}
    pub fn alert(text: &str) {
        eprintln!("Syn: {text}");
    }
}

fn main() {
    let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
    let profile = dir.join(".agent").join("app-profile");

    if core::appwin::console_is_syn(ADDR) {
        if !core::appwin::raise_existing() {
            core::appwin::open(URL, &profile, false);
        }
        return;
    }

    let ui = dir.join(if cfg!(windows) { "ui.exe" } else { "ui" });
    let mut cmd = Command::new(&ui);
    // Its working directory is where `.env`, `.agent` and the chats are.
    cmd.arg("--open").current_dir(&dir);
    sys::hide_console(&mut cmd, core::appwin::can_open());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            sys::alert(&format!("Syn could not start {}: {e}", ui.display()));
            return;
        }
    };

    // A `ui` that is still running after a few seconds has bound its port
    // and is serving. One that has gone has failed, and nobody would know.
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(4) {
        if let Ok(Some(status)) = child.try_wait() {
            sys::alert(&format!(
                "Syn stopped as it started ({status}).\n\nAnother program may be using port 7777. \
                 Running ui.exe from a terminal shows the reason."
            ));
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
