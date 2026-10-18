//! Putting Syn beside the application it works on, on Windows.
//!
//! The way to watch an agent work is Excel across two thirds of the screen
//! and Syn down the other third, edge to edge. Windows' own snap only
//! halves, and nobody drags two windows to an exact third and gets them flush
//! (each has an invisible border about seven pixels wide, so a hand-placed
//! pair is either a gap or an overlap). So the console can ask for it: which
//! side Syn takes, and what share of the screen's width; Syn's window and the
//! application's are then placed to match, on the screen Syn is on.
//!
//! The geometry is arithmetic and is tested everywhere. The calls that move
//! windows are Windows-only, and are proved against real Excel by
//! `scripts/live-layout.ps1`, which reads the rectangles back through UI
//! Automation.

use crate::json::{self, Value};

/// A rectangle in screen pixels, edges as Windows gives them (right and
/// bottom are one past the last pixel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// What the console asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Request {
    pub side: Side,
    /// Syn's share of the working area's width.
    pub share: f64,
}

/// Below a fifth Syn's window is narrower than the page can use, and above
/// four fifths the application beside it is a sliver; both are a slip of the
/// hand, not a layout, so they are refused rather than clamped.
pub const MIN_SHARE: f64 = 0.2;
pub const MAX_SHARE: f64 = 0.8;

const USAGE: &str = "POST /layout takes {\"side\":\"left\"|\"right\",\"share\":0.2 to 0.8} to arrange now (the side Syn takes and the share of the screen's width it gets), and/or {\"auto\":true|false} to have Syn and the apps it opens take their places by themselves";

/// What a request asks: to arrange now, to change whether Syn arranges by
/// itself, or both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ask {
    pub place: Option<Request>,
    pub auto: Option<bool>,
}

/// Read a request body. Closed like every other schema here: an unknown key
/// is refused, never ignored, and so is a share outside what makes a layout.
/// A side with no share (or the other way round) is half a layout: refused.
pub fn parse(body: &str) -> Result<Ask, String> {
    let v = json::parse(body).map_err(|e| format!("{USAGE} (that was not JSON: {e})"))?;
    let Some(pairs) = v.as_obj() else {
        return Err(USAGE.into());
    };
    if let Some((k, _)) = pairs.iter().find(|(k, _)| !["side", "share", "auto"].contains(&k.as_str())) {
        return Err(format!("{USAGE} (there is no key \"{k}\")"));
    }
    let auto = match v.get("auto") {
        None => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(format!("{USAGE} (auto must be true or false)")),
    };
    let place = match (v.get("side"), v.get("share")) {
        (None, None) => None,
        (Some(side), Some(share)) => {
            let side = match side.as_str() {
                Some("left") => Side::Left,
                Some("right") => Side::Right,
                _ => return Err(format!("{USAGE} (side must be \"left\" or \"right\")")),
            };
            let share = match share {
                Value::Num(n) => n.parse::<f64>().ok(),
                _ => None,
            };
            match share {
                Some(s) if (MIN_SHARE..=MAX_SHARE).contains(&s) => Some(Request { side, share: s }),
                _ => return Err(format!("{USAGE} (share must be a number from {MIN_SHARE} to {MAX_SHARE})")),
            }
        }
        _ => return Err(format!("{USAGE} (a layout needs both side and share)")),
    };
    if place.is_none() && auto.is_none() {
        return Err(USAGE.into());
    }
    Ok(Ask { place, auto })
}

/// What Syn remembers between runs, and what the apps it opens are held to.
/// Automatic is the default: the point of Syn is watching it work, and by hand
/// that is a window dragged next to a window, again, after every start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prefs {
    pub auto: bool,
    pub side: Side,
    pub share: f64,
}

/// A third of the screen, which is what most people mean by "beside it".
pub const DEFAULT_SHARE: f64 = 0.3333;

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { auto: true, side: Side::Right, share: DEFAULT_SHARE }
    }
}

impl Prefs {
    pub fn request(&self) -> Request {
        Request { side: self.side, share: self.share }
    }

    pub fn to_json(&self) -> String {
        let side = if self.side == Side::Left { "left" } else { "right" };
        format!("{{\"auto\":{},\"side\":\"{side}\",\"share\":{}}}", self.auto, self.share)
    }
}

/// Read saved preferences. A missing file, or a field that is not what it
/// should be, is the default for that field: a preference file is never a
/// reason for Syn not to start, or for a window to land off the screen.
pub fn parse_prefs(text: &str) -> Prefs {
    let mut p = Prefs::default();
    let Ok(v) = json::parse(text) else { return p };
    if let Some(Value::Bool(b)) = v.get("auto") {
        p.auto = *b;
    }
    match v.get("side").and_then(Value::as_str) {
        Some("left") => p.side = Side::Left,
        Some("right") => p.side = Side::Right,
        _ => {}
    }
    if let Some(Value::Num(n)) = v.get("share")
        && let Ok(s) = n.parse::<f64>()
        && (MIN_SHARE..=MAX_SHARE).contains(&s)
    {
        p.share = s;
    }
    p
}

/// Beside the chats and keys, in Syn's own data folder, so every process of
/// Syn (the console that is asked, the CLI that opens an application) reads
/// the same answer.
fn prefs_path() -> std::path::PathBuf {
    crate::chats::home().join("layout.json")
}

pub fn load() -> Prefs {
    std::fs::read_to_string(prefs_path()).map(|t| parse_prefs(&t)).unwrap_or_default()
}

fn save_to(path: &std::path::Path, p: &Prefs) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, p.to_json())
}

/// What the console answers to `GET /layout`: whether windows can be
/// arranged now, and what is remembered, which is what the menu shows.
pub fn status_json(can: bool, p: &Prefs) -> String {
    let side = if p.side == Side::Left { "left" } else { "right" };
    format!("{{\"can\":{can},\"auto\":{},\"side\":\"{side}\",\"share\":{}}}", p.auto, p.share)
}

/// Answer a request: arrange if it asks to (or turns automatic on, which is
/// asking to), and remember what it changes. A layout that could not be done
/// is not remembered as if it had been: the menu and the file must agree.
/// `arrange` is where the windows really move, passed in so the rest can be
/// tested without a screen.
fn handle_with(
    body: &str,
    store: &std::path::Path,
    mut prefs: Prefs,
    arrange: impl FnOnce(Request, bool) -> Result<String, String>,
) -> Result<String, String> {
    let ask = parse(body)?;
    let mut now = false;
    if let Some(r) = ask.place {
        prefs.side = r.side;
        prefs.share = r.share;
        now = true;
    }
    if let Some(a) = ask.auto {
        prefs.auto = a;
        now |= a;
    }
    // A side and a share asked for by hand may put any window beside Syn when
    // no Office application is open. Turning automatic on may not: it is
    // not a request to move the person's browser.
    let said = if now { Some(arrange(prefs.request(), ask.place.is_some())?) } else { None };
    save_to(store, &prefs).map_err(|e| format!("could not save the layout choice to {}: {e}", store.display()))?;
    Ok(match said {
        None => "Syn will leave the windows where you put them.".into(),
        Some(said) if prefs.auto => format!("{said} Syn and the apps it opens will keep to this."),
        Some(said) => said,
    })
}

/// `POST /layout`.
pub fn handle(body: &str) -> Result<String, String> {
    handle_with(body, &prefs_path(), load(), apply)
}

/// Syn's rectangle and the application's, for a Syn `syn_w` pixels wide on
/// `side` of `work`: together exactly the working area, touching, with no gap
/// and no overlap.
#[cfg(any(windows, test))]
fn split_at(work: Rect, side: Side, syn_w: i32) -> (Rect, Rect) {
    match side {
        Side::Right => (Rect { left: work.right - syn_w, ..work }, Rect { right: work.right - syn_w, ..work }),
        Side::Left => (Rect { right: work.left + syn_w, ..work }, Rect { left: work.left + syn_w, ..work }),
    }
}

/// The same for a share of the width.
#[cfg(any(windows, test))]
fn split(work: Rect, side: Side, share: f64) -> (Rect, Rect) {
    split_at(work, side, (f64::from(work.width()) * share).round() as i32)
}

/// How far a window's real edges lie outside what is drawn: the invisible
/// resize border and shadow Windows puts around a frame. Left, top, right,
/// bottom, from the difference between the window rectangle and the drawn
/// frame (`DWMWA_EXTENDED_FRAME_BOUNDS`).
#[cfg(any(windows, test))]
fn insets(window: Rect, drawn: Rect) -> (i32, i32, i32, i32) {
    (drawn.left - window.left, drawn.top - window.top, window.right - drawn.right, window.bottom - drawn.bottom)
}

/// The window rectangle to ask for so that what is drawn lands on `want`.
#[cfg(any(windows, test))]
fn ask_for(want: Rect, inset: (i32, i32, i32, i32)) -> Rect {
    Rect { left: want.left - inset.0, top: want.top - inset.1, right: want.right + inset.2, bottom: want.bottom + inset.3 }
}

/// Which window to put beside Syn, from what is known of each one.
#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
enum Pick {
    Office(&'static str),
    Ordinary,
    Skip,
}

#[cfg(any(windows, test))]
struct Facts<'a> {
    class: &'a str,
    title: &'a str,
    visible: bool,
    minimized: bool,
    /// On another virtual desktop, or a suspended app: listed, but not on screen.
    cloaked: bool,
    owned: bool,
    tool: bool,
    ours: bool,
}

/// The classes of the desktop itself and its taskbars: top-level, visible,
/// titled, and nobody's idea of a window to put a chat beside.
#[cfg(any(windows, test))]
const SHELL: [&str; 4] = ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"];

#[cfg(any(windows, test))]
fn office_app(class: &str) -> Option<&'static str> {
    match class {
        "XLMAIN" => Some("Excel"),
        "OpusApp" => Some("Word"),
        "PPTFrameClass" => Some("PowerPoint"),
        _ => None,
    }
}

/// An Office window counts even when minimized (Syn drives Excel while it is
/// down in the taskbar, and asking to see them together should bring it up);
/// any other window must be one a person could be looking at.
#[cfg(any(windows, test))]
fn classify(f: &Facts) -> Pick {
    if f.ours || !f.visible || f.cloaked || f.owned || f.tool || f.title.is_empty() || SHELL.contains(&f.class) {
        return Pick::Skip;
    }
    if let Some(app) = office_app(f.class) {
        return Pick::Office(app);
    }
    if f.minimized { Pick::Skip } else { Pick::Ordinary }
}

/// "33%" for 0.33, "67%" for 0.666: whole percents, which is all a person sets.
#[cfg(any(windows, test))]
fn percent(share: f64) -> String {
    format!("{}%", (share * 100.0).round() as i32)
}

/// "right with 33% of the screen".
#[cfg(any(windows, test))]
fn slot_words(req: Request) -> String {
    let side = match req.side {
        Side::Left => "left",
        Side::Right => "right",
    };
    format!("{side} with {} of the screen", percent(req.share))
}

/// What the console tells the person after a layout.
#[cfg(any(windows, test))]
fn describe(req: Request, beside: Option<&str>, crowded_by: i32) -> String {
    let slot = slot_words(req);
    let mut out = match beside {
        Some(who) => format!("Syn is on the {slot}, and {who} has the rest."),
        None => format!("Syn is on the {slot}. Nothing else was open to put beside it."),
    };
    if crowded_by > 0 {
        let who = beside.unwrap_or("The other window");
        out.push_str(&format!(" {who} would not go that narrow, so it reaches {crowded_by} pixels under Syn: choose a smaller share for Syn."));
    }
    out
}

/// Whether a layout can be done right now: there is a Syn window of Syn's own
/// to move. A console in a browser tab has none, and the page hides the
/// controls rather than offering what cannot be done.
pub fn available() -> bool {
    #[cfg(windows)]
    {
        !crate::appwin::win::syn_windows().is_empty()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Place Syn's window and the application beside it. The text says what was
/// done, or why nothing was. With `anything`, when no Excel, Word or
/// PowerPoint is open the window the person was in before Syn goes beside it
/// instead; without, only Office does (and with none, Syn alone takes its
/// place).
pub fn apply(req: Request, anything: bool) -> Result<String, String> {
    #[cfg(windows)]
    {
        win::apply_with(req, if anything { win::Partner::Pick } else { win::Partner::Office })
    }
    #[cfg(not(windows))]
    {
        let _ = (req, anything);
        Err("Arranging windows is only done on Windows.".into())
    }
}

/// The application Syn has just opened a document in is given its place
/// beside Syn, by itself, if the person has not turned that off. Returns at
/// once: the window is not always up when `open` returns (Excel has been seen
/// to show a start screen first), so a thread waits for it, a few seconds at
/// most. Each window is placed once, the first time Syn meets it; after that
/// it is the person's to move, and a second `open` does not pull it back.
pub fn follow(app: &str) {
    #[cfg(windows)]
    {
        win::follow(app);
    }
    #[cfg(not(windows))]
    {
        let _ = app;
    }
}

/// Syn's own window takes its place when Syn starts, if automatic layout is
/// on, and an Excel, Word or PowerPoint window already open on the same
/// screen takes the rest (maximized or not; minimized ones and ones on
/// another monitor are left alone). The window program restores wherever it
/// was last closed, a moment after it appears; this waits for it and then
/// puts it in the chosen slot.
pub fn settle_self() {
    #[cfg(windows)]
    {
        win::settle_self();
    }
}

/// The class of an application's main window, for the apps Syn opens
/// documents in (`desk` calls them excel, word and ppt).
#[cfg(any(windows, test))]
fn office_class(app: &str) -> Option<&'static str> {
    match app {
        "excel" => Some("XLMAIN"),
        "word" => Some("OpusApp"),
        "ppt" | "powerpoint" => Some("PPTFrameClass"),
        _ => None,
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::appwin::win as shared;
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Raw {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    impl From<Raw> for Rect {
        fn from(r: Raw) -> Rect {
            Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
        }
    }

    #[repr(C)]
    struct MonitorInfo {
        size: u32,
        monitor: Raw,
        work: Raw,
        flags: u32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(cb: unsafe extern "system" fn(isize, isize) -> i32, lparam: isize) -> i32;
        fn IsWindow(h: isize) -> i32;
        fn IsWindowVisible(h: isize) -> i32;
        fn IsIconic(h: isize) -> i32;
        fn IsZoomed(h: isize) -> i32;
        fn ShowWindow(h: isize, cmd: i32) -> i32;
        fn SetWindowPos(h: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
        fn GetWindowRect(h: isize, r: *mut Raw) -> i32;
        fn GetWindow(h: isize, cmd: u32) -> isize;
        fn GetWindowLongW(h: isize, index: i32) -> i32;
        fn MonitorFromWindow(h: isize, flags: u32) -> isize;
        fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
        fn SetThreadDpiAwarenessContext(ctx: isize) -> isize;
        fn FlashWindowEx(info: *const FlashInfo) -> i32;
    }

    /// FLASHWINFO. `flags` 0 is FLASHW_STOP.
    #[repr(C)]
    struct FlashInfo {
        size: u32,
        hwnd: isize,
        flags: u32,
        count: u32,
        timeout: u32,
    }

    /// Stop `h`'s taskbar button from flashing. An Office window opened by
    /// the helper, while the person types in Syn, cannot take the foreground,
    /// so Windows flashes its button instead, and a taskbar set to hide
    /// itself slides up and stays up until the window is used: it ruined a
    /// recording (measured: it rose 2 s after the open and was still up 20 s
    /// later). Stopping the flash puts the taskbar back and leaves focus
    /// where the person is typing. A window that is not flashing is
    /// unaffected.
    fn calm(h: isize) {
        let info = FlashInfo { size: std::mem::size_of::<FlashInfo>() as u32, hwnd: h, flags: 0, count: 0, timeout: 0 };
        // SAFETY: `info` is a FLASHWINFO with its size set, as the call needs;
        // a window that has gone makes the call fail, which is not a fault.
        unsafe { FlashWindowEx(&info) };
    }

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmGetWindowAttribute(h: isize, attr: u32, out: *mut c_void, size: u32) -> i32;
    }

    const SW_RESTORE: i32 = 9;
    const GW_OWNER: u32 = 4;
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TOOLWINDOW: i32 = 0x80;
    const WS_EX_NOACTIVATE: i32 = 0x0800_0000;
    const MONITOR_DEFAULTTONEAREST: u32 = 2;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const HWND_TOP: isize = 0;
    const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;
    const DWMWA_CLOAKED: u32 = 14;
    /// DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2.
    const PER_MONITOR_V2: isize = -4;

    /// This thread sees real pixels while it works. A process that is not DPI
    /// aware is lied to about every coordinate on a scaled screen, and windows
    /// of other processes would land at the wrong place by the scale factor.
    struct Pixels(isize);

    impl Pixels {
        fn real() -> Pixels {
            // SAFETY: a plain call that changes only this thread's setting.
            Pixels(unsafe { SetThreadDpiAwarenessContext(PER_MONITOR_V2) })
        }
    }

    impl Drop for Pixels {
        fn drop(&mut self) {
            if self.0 != 0 {
                // SAFETY: puts back the value this thread had.
                unsafe { SetThreadDpiAwarenessContext(self.0) };
            }
        }
    }

    fn window_rect(h: isize) -> Rect {
        let mut r = Raw::default();
        // SAFETY: `r` is a RECT-shaped value the call fills in.
        unsafe { GetWindowRect(h, &mut r) };
        r.into()
    }

    /// The rectangle that is drawn, without the invisible border; the window
    /// rectangle when Windows will not say.
    fn drawn_rect(h: isize) -> Rect {
        let mut r = Raw::default();
        // SAFETY: `r` is 16 bytes, the size given.
        let hr = unsafe { DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut Raw as *mut c_void, 16) };
        if hr == 0 { r.into() } else { window_rect(h) }
    }

    fn cloaked(h: isize) -> bool {
        let mut v = 0u32;
        // SAFETY: `v` is 4 bytes, the size given.
        let hr = unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut v as *mut u32 as *mut c_void, 4) };
        hr == 0 && v != 0
    }

    /// The part of the screen windows can use on the monitor `h` is on: the
    /// monitor less the taskbar.
    fn work_area(h: isize) -> Option<Rect> {
        let mut info = MonitorInfo { size: std::mem::size_of::<MonitorInfo>() as u32, monitor: Raw::default(), work: Raw::default(), flags: 0 };
        // SAFETY: `info` has its size set, as the call needs.
        let ok = unsafe {
            let m = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
            m != 0 && GetMonitorInfoW(m, &mut info) != 0
        };
        ok.then(|| info.work.into())
    }

    /// Move `h` so that what is drawn of it lands on `want`; returns where it
    /// really landed, because a window with a minimum size does not go
    /// narrower than that however it is asked. Brought to the front without
    /// being activated, which another process may do and may not steal focus.
    fn place(h: isize, want: Rect) -> Rect {
        // SAFETY: plain calls on a window handle; a window that has gone since
        // it was listed makes them fail, which is not a fault here.
        unsafe {
            if IsZoomed(h) != 0 || IsIconic(h) != 0 {
                ShowWindow(h, SW_RESTORE);
            }
            // Already there: leave it be, so asking again (a second `open`, a
            // second click) does not make the window flicker.
            let now = drawn_rect(h);
            if (now.left - want.left).abs() <= 1
                && (now.right - want.right).abs() <= 1
                && (now.top - want.top).abs() <= 1
                && (now.bottom - want.bottom).abs() <= 1
            {
                return now;
            }
            let at = ask_for(want, insets(window_rect(h), now));
            SetWindowPos(h, HWND_TOP, at.left, at.top, at.width(), at.height(), SWP_NOACTIVATE);
        }
        drawn_rect(h)
    }

    struct Seen {
        top: Vec<isize>,
    }

    unsafe extern "system" fn collect(h: isize, lparam: isize) -> i32 {
        // SAFETY: `lparam` is the `Seen` `windows` passed, alive for the whole
        // of the EnumWindows call that calls this.
        let seen = unsafe { &mut *(lparam as *mut Seen) };
        if unsafe { IsWindowVisible(h) } != 0 {
            seen.top.push(h);
        }
        1
    }

    /// One top-level window, as far as choosing a partner goes.
    struct Candidate {
        h: isize,
        pick: Pick,
        class: String,
        title: String,
    }

    /// Every visible top-level window, topmost first, with what to make of it.
    fn candidates(ours: &[isize]) -> Vec<Candidate> {
        let mut seen = Seen { top: Vec::new() };
        // SAFETY: `seen` outlives the call.
        unsafe { EnumWindows(collect, &mut seen as *mut Seen as isize) };
        seen.top
            .into_iter()
            .map(|h| {
                let (class, title) = (shared::class(h), shared::title(h));
                // SAFETY: plain calls on a window handle.
                let (minimized, owned, ex) =
                    unsafe { (IsIconic(h) != 0, GetWindow(h, GW_OWNER) != 0, GetWindowLongW(h, GWL_EXSTYLE)) };
                let pick = classify(&Facts {
                    class: &class,
                    title: &title,
                    visible: true,
                    minimized,
                    cloaked: cloaked(h),
                    owned,
                    tool: ex & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) != 0,
                    ours: ours.contains(&h),
                });
                Candidate { h, pick, class, title }
            })
            .collect()
    }

    /// The topmost Office window that is already on Syn's own screen and is
    /// not minimized: what is there to be given the rest when Syn starts.
    /// Syn taking its third on top of a maximized Excel, and leaving Excel
    /// full screen behind it, was seen to hide a third of the workbook until
    /// the first `open`. A window on the person's other monitor is their own
    /// arrangement, and one they minimized is theirs to bring back; neither
    /// is touched without being asked.
    fn partner_here(ours: &[isize], syn: isize) -> Option<(isize, String)> {
        let screen = monitor_of(syn);
        candidates(ours).into_iter().find_map(|c| match c.pick {
            // SAFETY: plain call on a window handle.
            Pick::Office(app) if unsafe { IsIconic(c.h) } == 0 && monitor_of(c.h) == screen => Some((c.h, app.to_string())),
            _ => None,
        })
    }

    fn monitor_of(h: isize) -> isize {
        // SAFETY: a plain call on a window handle.
        unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) }
    }

    /// The window to put beside Syn: the topmost Office window, else the
    /// topmost ordinary one. Office first because Syn works on those, so
    /// "beside Syn" means beside what it is doing; the window just behind
    /// Syn otherwise, which is where the person was before they came to it.
    fn partner(ours: &[isize], anything: bool) -> Option<(isize, String)> {
        let all = candidates(ours);
        for c in &all {
            if let Pick::Office(app) = c.pick {
                return Some((c.h, app.to_string()));
            }
        }
        if !anything {
            return None;
        }
        all.iter().find(|c| c.pick == Pick::Ordinary).map(|c| {
            let shown: String = c.title.chars().take(40).collect();
            (c.h, format!("\u{201c}{shown}\u{201d}"))
        })
    }

    /// The topmost window of `class` that a person could be looking at.
    fn window_of(class: &str, ours: &[isize]) -> Option<(isize, String)> {
        candidates(ours).into_iter().find_map(|c| match c.pick {
            Pick::Office(app) if c.class == class => Some((c.h, app.to_string())),
            _ => None,
        })
    }

    /// Which application's window goes beside Syn.
    pub enum Partner {
        /// The topmost Office window, else the topmost ordinary one: what a
        /// side or a share clicked in the menu means.
        Pick,
        /// The topmost Office window, else none: what turning automatic on
        /// means, which is no request to move anything but Syn and Office.
        Office,
        /// This one, which Syn has just opened a document in.
        Window(isize, String),
        /// The topmost Office window already on Syn's screen, not minimized:
        /// what Syn starting means.
        Here,
    }

    /// What one arrangement did: the words for the menu, and the other
    /// window with the place it was given, when there was one.
    struct Done {
        text: String,
        partner: Option<(isize, Rect)>,
    }

    pub fn apply_with(req: Request, with: Partner) -> Result<String, String> {
        arrange(req, with).map(|done| done.text)
    }

    fn arrange(req: Request, with: Partner) -> Result<Done, String> {
        let _px = Pixels::real();
        let ours = shared::syn_windows();
        let Some(&syn) = ours.first() else {
            return Err("Syn's own window is not on screen: a console open in a browser tab cannot be moved. Open Syn from its shortcut and try again.".into());
        };
        // Restored first: where a maximized window's screen is, is not a
        // question with one answer.
        // SAFETY: plain calls on a window handle.
        unsafe {
            if IsZoomed(syn) != 0 || IsIconic(syn) != 0 {
                ShowWindow(syn, SW_RESTORE);
            }
        }
        let work = work_area(syn).ok_or("Windows would not say how big this screen is.")?;
        let (want_syn, _) = split(work, req.side, req.share);
        let mut got = place(syn, want_syn);
        // Syn's window has a smallest size. If it did not go as narrow as
        // asked it stands where it spilled to; put it flush on its side at
        // the width it really has, and give the other window what is left.
        if got.width() != want_syn.width() {
            got = place(syn, split_at(work, req.side, got.width()).0);
        }
        let (_, rest) = split_at(work, req.side, got.width());
        let beside = match with {
            Partner::Pick => partner(&ours, true),
            Partner::Office => partner(&ours, false),
            Partner::Window(h, name) => Some((h, name)),
            Partner::Here => partner_here(&ours, syn),
        };
        let mut crowded = 0;
        if let Some((h, _)) = &beside {
            let landed = place(*h, rest);
            crowded = (landed.width() - rest.width()).max(0);
        }
        // Syn last, so it is the one in front where they meet, and the one
        // the person is still talking to.
        // SAFETY: plain calls on a window handle.
        unsafe { SetWindowPos(syn, HWND_TOP, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOMOVE) };
        // Not `shared::raise(syn)` (SetForegroundWindow) as it was: this runs in
        // Syn's helper, which is not the process the person is typing in, so
        // Windows refuses to hand it the foreground and flashes Syn's taskbar
        // button instead, and a taskbar set to hide itself slid up at every
        // Excel or Word open (and again each time Syn started). Syn is already
        // in front where the two meet, from the call above; what the helper may
        // do is only make sure its button is not flashing.
        calm(syn);
        Ok(Done {
            text: describe(req, beside.as_ref().map(|(_, who)| who.as_str()), crowded),
            partner: beside.map(|(h, _)| (h, rest)),
        })
    }

    /// How long, after Syn gives an application's window its place, a window
    /// that leaves it is put back.
    const HOLD: std::time::Duration = std::time::Duration::from_millis(2500);

    /// Excel and Word put their window back to the size it had when they were
    /// last closed once the first document is up, which is after Syn has
    /// placed it: Excel came out 7 px short of the screen with Syn's third
    /// beside it, though the same call had put it flush a run before. The
    /// person has not touched the window yet in these first seconds (the
    /// application did it within about half a second of its document
    /// appearing), so a window that is no longer where it was left is placed
    /// again; after them it is theirs. One that cannot be as narrow as asked is
    /// judged by where it really landed, not by what was asked, so it is not
    /// chased.
    fn hold(h: isize, want: Rect) {
        use std::time::{Duration, Instant};
        let _px = Pixels::real();
        // Placed again at once, so a window that already moved since `arrange`
        // is caught, and a window that is in place is left alone.
        let mut landed = place(h, want);
        calm(h);
        let until = Instant::now() + HOLD;
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(300));
            // SAFETY: a plain call on a window handle.
            if unsafe { IsWindow(h) } == 0 {
                return;
            }
            calm(h);
            let now = drawn_rect(h);
            if now != landed {
                landed = place(h, want);
            }
        }
    }

    /// Windows already given their place this run; a person who moves one
    /// afterwards is not argued with.
    static PLACED: std::sync::Mutex<Vec<isize>> = std::sync::Mutex::new(Vec::new());

    fn first_time(h: isize) -> bool {
        let Ok(mut placed) = PLACED.lock() else { return false };
        if placed.contains(&h) {
            return false;
        }
        placed.push(h);
        true
    }

    pub fn follow(app: &str) {
        use std::time::{Duration, Instant};
        let Some(class) = office_class(app) else { return };
        // Never from this crate's own tests: they open documents through
        // fake hands, and on a developer's machine with Syn and Excel on the
        // screen would move the real windows.
        if cfg!(test) || !load().auto {
            return;
        }
        std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(12);
            while Instant::now() < until {
                // Read again each time: it may have been turned off since.
                let prefs = load();
                if !prefs.auto {
                    return;
                }
                if available() {
                    let _px = Pixels::real();
                    if let Some((h, name)) = window_of(class, &shared::syn_windows()) {
                        // First thing, before anything is measured: every
                        // moment the button flashes the taskbar is up.
                        calm(h);
                        if first_time(h)
                            && let Ok(Done { partner: Some((p, rest)), .. }) = arrange(prefs.request(), Partner::Window(h, name))
                        {
                            hold(p, rest);
                        }
                        return;
                    }
                }
                // Short: the window is looked for the moment it can exist, so
                // its flash is stopped before the taskbar has finished rising.
                std::thread::sleep(Duration::from_millis(40));
            }
        });
    }

    pub fn settle_self() {
        use std::time::{Duration, Instant};
        if cfg!(test) {
            return;
        }
        std::thread::spawn(|| {
            let until = Instant::now() + Duration::from_secs(25);
            while Instant::now() < until {
                if !load().auto {
                    return;
                }
                if available() {
                    // The window program puts itself where it was last closed
                    // as it starts; wait that out, then take the slot, and
                    // give an Office window already on this screen the rest.
                    std::thread::sleep(Duration::from_millis(700));
                    let prefs = load();
                    if prefs.auto
                        && let Ok(Done { partner: Some((p, rest)), .. }) = arrange(prefs.request(), Partner::Here)
                    {
                        hold(p, rest);
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect { left: 0, top: 0, right: 1920, bottom: 1032 };

    fn req(body: &str) -> Result<Ask, String> {
        parse(body)
    }

    fn placed(side: Side, share: f64) -> Option<Request> {
        Some(Request { side, share })
    }

    #[test]
    fn syn_on_the_right_for_a_third_leaves_excel_two_thirds_touching_it() {
        let (syn, other) = split(WORK, Side::Right, 1.0 / 3.0);
        assert_eq!(syn, Rect { left: 1280, top: 0, right: 1920, bottom: 1032 });
        assert_eq!(other, Rect { left: 0, top: 0, right: 1280, bottom: 1032 });
    }

    #[test]
    fn syn_on_the_left_mirrors_it() {
        let (syn, other) = split(WORK, Side::Left, 0.25);
        assert_eq!(syn, Rect { left: 0, top: 0, right: 480, bottom: 1032 });
        assert_eq!(other, Rect { left: 480, top: 0, right: 1920, bottom: 1032 });
    }

    #[test]
    fn the_two_always_fill_the_working_area_with_no_gap_and_no_overlap() {
        // A second monitor to the left of the primary has a negative origin,
        // and a width that does not divide evenly must not lose a pixel.
        let work = Rect { left: -1366, top: 24, right: 1, bottom: 768 };
        for side in [Side::Left, Side::Right] {
            for share in [0.2, 0.25, 0.33, 0.4, 0.5, 0.67, 0.8] {
                let (syn, other) = split(work, side, share);
                let (a, b) = if side == Side::Left { (syn, other) } else { (other, syn) };
                assert_eq!(a.left, work.left, "{side:?} {share}");
                assert_eq!(b.right, work.right, "{side:?} {share}");
                assert_eq!(a.right, b.left, "touching: {side:?} {share}");
                assert_eq!((syn.top, syn.bottom, other.top, other.bottom), (work.top, work.bottom, work.top, work.bottom));
            }
        }
    }

    #[test]
    fn a_syn_that_would_not_go_narrow_leaves_the_other_window_what_is_left() {
        // Asked for 341 wide, Syn stayed at 660: the rest is what is left.
        let (syn, other) = split_at(WORK, Side::Right, 660);
        assert_eq!((syn.left, syn.width()), (1260, 660));
        assert_eq!(other.right, 1260);
    }

    #[test]
    fn the_invisible_border_is_taken_out_so_what_is_drawn_lands_flush() {
        // A window whose frame is drawn 7px inside its left, right and
        // bottom edges, as a resizable window is on Windows 10 and 11.
        let window = Rect { left: 100, top: 100, right: 700, bottom: 507 };
        let drawn = Rect { left: 107, top: 100, right: 693, bottom: 500 };
        let inset = insets(window, drawn);
        assert_eq!(inset, (7, 0, 7, 7));
        let want = Rect { left: 1280, top: 0, right: 1920, bottom: 1032 };
        assert_eq!(ask_for(want, inset), Rect { left: 1273, top: 0, right: 1927, bottom: 1039 });
        // And a window with no border asks for exactly what it wants.
        assert_eq!(ask_for(want, (0, 0, 0, 0)), want);
    }

    fn facts<'a>(class: &'a str, title: &'a str) -> Facts<'a> {
        Facts { class, title, visible: true, minimized: false, cloaked: false, owned: false, tool: false, ours: false }
    }

    #[test]
    fn office_windows_are_found_by_their_class_and_named() {
        assert_eq!(classify(&facts("XLMAIN", "Book1 - Excel")), Pick::Office("Excel"));
        assert_eq!(classify(&facts("OpusApp", "Report - Word")), Pick::Office("Word"));
        assert_eq!(classify(&facts("PPTFrameClass", "Deck - PowerPoint")), Pick::Office("PowerPoint"));
        // Down in the taskbar, Excel is still the one to bring up.
        let mut f = facts("XLMAIN", "Book1 - Excel");
        f.minimized = true;
        assert_eq!(classify(&f), Pick::Office("Excel"));
    }

    #[test]
    fn what_a_person_could_not_be_looking_at_is_never_put_beside_syn() {
        let skip = |edit: &dyn Fn(&mut Facts)| {
            let mut f = facts("Notepad", "notes.txt - Notepad");
            edit(&mut f);
            classify(&f) == Pick::Skip
        };
        assert_eq!(classify(&facts("Notepad", "notes.txt - Notepad")), Pick::Ordinary);
        assert!(skip(&|f| f.ours = true), "Syn itself");
        assert!(skip(&|f| f.cloaked = true), "another virtual desktop");
        assert!(skip(&|f| f.minimized = true), "minimized");
        assert!(skip(&|f| f.owned = true), "a dialog");
        assert!(skip(&|f| f.tool = true), "a tool window");
        assert!(skip(&|f| f.title = ""), "no title");
        assert!(skip(&|f| f.class = "Progman"), "the desktop");
        assert!(skip(&|f| f.class = "Shell_TrayWnd"), "the taskbar");
    }

    #[test]
    fn a_request_names_a_side_and_a_share_or_whether_to_arrange_by_itself() {
        assert_eq!(req(r#"{"side":"right","share":0.33}"#).unwrap(), Ask { place: placed(Side::Right, 0.33), auto: None });
        assert_eq!(req(r#"{"share":0.5,"side":"left"}"#).unwrap().place, placed(Side::Left, 0.5));
        assert_eq!(req(r#"{"auto":false}"#).unwrap(), Ask { place: None, auto: Some(false) });
        assert_eq!(
            req(r#"{"side":"left","share":0.4,"auto":true}"#).unwrap(),
            Ask { place: placed(Side::Left, 0.4), auto: Some(true) }
        );
        // The ends are layouts; either side of them is a slip.
        assert!(req(r#"{"side":"left","share":0.2}"#).is_ok());
        assert!(req(r#"{"side":"left","share":0.8}"#).is_ok());
        for bad in [
            r#"{"side":"left","share":0.19}"#,
            r#"{"side":"left","share":0.81}"#,
            r#"{"side":"left","share":"0.5"}"#,
            r#"{"side":"left"}"#,
            r#"{"share":0.5}"#,
            r#"{"side":"top","share":0.5}"#,
            r#"{"side":"left","share":0.5,"monitor":2}"#,
            r#"{"auto":"yes"}"#,
            r#"{}"#,
            r#"[1]"#,
            "not json",
            "",
        ] {
            let e = req(bad).unwrap_err();
            // Each refusal says what a good call looks like.
            assert!(e.contains(r#"{"side":"left"|"right","share":0.2 to 0.8}"#), "{bad}: {e}");
            assert!(e.contains(r#"{"auto":true|false}"#), "{bad}: {e}");
        }
    }

    #[test]
    fn syn_arranges_by_itself_unless_told_not_to_and_a_bad_file_is_never_a_reason_to_stop() {
        let d = Prefs::default();
        assert_eq!((d.auto, d.side, d.share), (true, Side::Right, DEFAULT_SHARE));
        // A third of a 1920 screen is 640 exactly, not 634.
        assert_eq!(split(WORK, d.side, d.share).0.width(), 640);
        let saved = Prefs { auto: false, side: Side::Left, share: 0.5 };
        assert_eq!(parse_prefs(&saved.to_json()), saved, "what is written is what is read");
        // Fields that are missing or wrong are the default for that field only.
        assert_eq!(parse_prefs(r#"{"auto":false}"#), Prefs { auto: false, ..d });
        assert_eq!(parse_prefs(r#"{"side":"left","share":9}"#), Prefs { side: Side::Left, ..d });
        assert_eq!(parse_prefs(r#"{"side":"up","auto":"maybe"}"#), d);
        for junk in ["", "not json", "[1,2]", "null"] {
            assert_eq!(parse_prefs(junk), d, "{junk:?}");
        }
    }

    /// A folder of its own for a test that writes the choice.
    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("syn-layout-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_click_arranges_and_is_remembered_and_the_toggle_alone_only_changes_the_choice() {
        let dir = scratch("handle");
        let file = dir.join("layout.json");
        let moved = std::cell::RefCell::new(Vec::new());
        let arrange = |r: Request, anything: bool| {
            moved.borrow_mut().push((r, anything));
            Ok::<_, String>("Syn is on the left with 50% of the screen, and Excel has the rest.".to_string())
        };

        // Turning it off is a setting, not a move.
        let said = handle_with(r#"{"auto":false}"#, &file, Prefs::default(), arrange).unwrap();
        assert!(said.contains("leave the windows"), "{said}");
        assert!(moved.borrow().is_empty());
        assert!(!parse_prefs(&std::fs::read_to_string(&file).unwrap()).auto);

        // A side and a share arrange now, and are kept, with automatic as it was.
        let off = parse_prefs(&std::fs::read_to_string(&file).unwrap());
        let said = handle_with(r#"{"side":"left","share":0.5}"#, &file, off, arrange).unwrap();
        assert!(said.starts_with("Syn is on the left with 50%"), "{said}");
        assert!(!said.contains("keep to this"), "automatic is off, so nothing is promised: {said}");
        assert_eq!(moved.borrow().as_slice(), [(Request { side: Side::Left, share: 0.5 }, true)], "by hand, any window will do");
        assert_eq!(parse_prefs(&std::fs::read_to_string(&file).unwrap()), Prefs { auto: false, side: Side::Left, share: 0.5 });

        // Turning it on is asking for it: it arranges at the remembered place.
        let kept = parse_prefs(&std::fs::read_to_string(&file).unwrap());
        let said = handle_with(r#"{"auto":true}"#, &file, kept, arrange).unwrap();
        assert!(said.ends_with("Syn and the apps it opens will keep to this."), "{said}");
        assert_eq!(
            moved.borrow().last(),
            Some(&(Request { side: Side::Left, share: 0.5 }, false)),
            "turning it on moves Syn and Office, not the person's browser"
        );
        assert!(parse_prefs(&std::fs::read_to_string(&file).unwrap()).auto);

        // A refused request changes nothing.
        let before = std::fs::read_to_string(&file).unwrap();
        assert!(handle_with(r#"{"share":0.9,"side":"left"}"#, &file, Prefs::default(), arrange).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_layout_that_could_not_be_done_is_not_remembered_as_if_it_had_been() {
        // The menu shows the button that was pressed only when the console
        // did it, and the file the next start reads must say the same.
        let dir = scratch("nowindow");
        let file = dir.join("layout.json");
        let e = handle_with(r#"{"side":"left","share":0.4}"#, &file, Prefs::default(), |_, _| Err("no window".to_string())).unwrap_err();
        assert_eq!(e, "no window");
        assert!(!file.exists(), "nothing was saved");
        let e = handle_with(r#"{"auto":true}"#, &file, Prefs { auto: false, ..Prefs::default() }, |_, _| Err("no window".to_string())).unwrap_err();
        assert_eq!(e, "no window");
        assert!(!file.exists());
        // Turning it off needs no window, and is kept.
        handle_with(r#"{"auto":false}"#, &file, Prefs::default(), |_, _| Err("not called".to_string())).unwrap();
        assert!(!parse_prefs(&std::fs::read_to_string(&file).unwrap()).auto);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_menu_is_told_what_can_be_done_and_what_is_remembered() {
        let p = Prefs { auto: true, side: Side::Left, share: 0.5 };
        assert_eq!(status_json(true, &p), r#"{"can":true,"auto":true,"side":"left","share":0.5}"#);
        assert_eq!(status_json(false, &Prefs::default()), r#"{"can":false,"auto":true,"side":"right","share":0.3333}"#);
    }

    #[test]
    fn the_office_apps_syn_opens_documents_in_are_found_by_their_main_window() {
        assert_eq!(office_class("excel"), Some("XLMAIN"));
        assert_eq!(office_class("word"), Some("OpusApp"));
        assert_eq!(office_class("ppt"), Some("PPTFrameClass"));
        // A page or a window of someone else's is not an application Syn
        // gives a place to by itself.
        assert_eq!(office_class("web"), None);
        assert_eq!(office_class("ui"), None);
    }

    #[test]
    fn the_person_is_told_what_was_done_and_what_could_not_be() {
        let r = Request { side: Side::Right, share: 1.0 / 3.0 };
        assert_eq!(describe(r, Some("Excel"), 0), "Syn is on the right with 33% of the screen, and Excel has the rest.");
        assert!(describe(r, None, 0).contains("Nothing else was open"));
        let crowded = describe(Request { side: Side::Left, share: 0.67 }, Some("Word"), 120);
        assert!(crowded.starts_with("Syn is on the left with 67% of the screen, and Word has the rest."), "{crowded}");
        assert!(crowded.contains("Word would not go that narrow, so it reaches 120 pixels under Syn"), "{crowded}");
    }

    #[cfg(not(windows))]
    #[test]
    fn off_windows_there_is_nothing_to_arrange_and_the_page_is_not_offered_it() {
        assert!(!available());
        assert!(apply(Request { side: Side::Right, share: 0.33 }, true).unwrap_err().contains("only done on Windows"));
        // Following an application off Windows is nothing at all, not an error.
        follow("excel");
    }
}
