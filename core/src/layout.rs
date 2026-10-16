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

const USAGE: &str =
    "POST /layout takes {\"side\":\"left\"|\"right\",\"share\":0.2 to 0.8}: the side Syn takes and the share of the screen's width it gets";

/// Read a request body. Closed like every other schema here: an unknown key
/// is refused, never ignored, and so is a share outside what makes a layout.
pub fn parse(body: &str) -> Result<Request, String> {
    let v = json::parse(body).map_err(|e| format!("{USAGE} (that was not JSON: {e})"))?;
    let Some(pairs) = v.as_obj() else {
        return Err(USAGE.into());
    };
    if let Some((k, _)) = pairs.iter().find(|(k, _)| k != "side" && k != "share") {
        return Err(format!("{USAGE} (there is no key \"{k}\")"));
    }
    let side = match v.get("side").and_then(Value::as_str) {
        Some("left") => Side::Left,
        Some("right") => Side::Right,
        _ => return Err(format!("{USAGE} (side must be \"left\" or \"right\")")),
    };
    let share = match v.get("share") {
        Some(Value::Num(n)) => n.parse::<f64>().ok(),
        _ => None,
    };
    match share {
        Some(s) if (MIN_SHARE..=MAX_SHARE).contains(&s) => Ok(Request { side, share: s }),
        _ => Err(format!("{USAGE} (share must be a number from {MIN_SHARE} to {MAX_SHARE})")),
    }
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

/// What the console tells the person after a layout.
#[cfg(any(windows, test))]
fn describe(req: Request, beside: Option<&str>, crowded_by: i32) -> String {
    let side = match req.side {
        Side::Left => "left",
        Side::Right => "right",
    };
    let mut out = match beside {
        Some(who) => format!("Syn is on the {side} with {} of the screen, and {who} has the rest.", percent(req.share)),
        None => format!(
            "Syn is on the {side} with {} of the screen. Nothing else was open to put beside it: open Excel, Word or PowerPoint and ask again.",
            percent(req.share)
        ),
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
/// done, or why nothing was.
pub fn apply(req: Request) -> Result<String, String> {
    #[cfg(windows)]
    {
        win::apply(req)
    }
    #[cfg(not(windows))]
    {
        let _ = req;
        Err("Arranging windows is only done on Windows.".into())
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
            let at = ask_for(want, insets(window_rect(h), drawn_rect(h)));
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

    /// The window to put beside Syn: the topmost Office window, else the
    /// topmost ordinary one. Office first because Syn works on those, so
    /// "beside Syn" means beside what it is doing; the window just behind
    /// Syn otherwise, which is where the person was before they came to it.
    fn partner(ours: &[isize]) -> Option<(isize, String)> {
        let mut seen = Seen { top: Vec::new() };
        // SAFETY: `seen` outlives the call.
        unsafe { EnumWindows(collect, &mut seen as *mut Seen as isize) };
        let mut ordinary = None;
        for h in seen.top {
            let (class, title) = (shared::class(h), shared::title(h));
            // SAFETY: plain calls on a window handle.
            let (minimized, owned, ex) = unsafe { (IsIconic(h) != 0, GetWindow(h, GW_OWNER) != 0, GetWindowLongW(h, GWL_EXSTYLE)) };
            let facts = Facts {
                class: &class,
                title: &title,
                visible: true,
                minimized,
                cloaked: cloaked(h),
                owned,
                tool: ex & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) != 0,
                ours: ours.contains(&h),
            };
            match classify(&facts) {
                Pick::Office(app) => return Some((h, app.to_string())),
                Pick::Ordinary if ordinary.is_none() => {
                    let shown: String = title.chars().take(40).collect();
                    ordinary = Some((h, format!("\u{201c}{shown}\u{201d}")));
                }
                _ => {}
            }
        }
        ordinary
    }

    pub fn apply(req: Request) -> Result<String, String> {
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
        let beside = partner(&ours);
        let mut crowded = 0;
        if let Some((h, _)) = &beside {
            let landed = place(*h, rest);
            crowded = (landed.width() - rest.width()).max(0);
        }
        // Syn last, so it is the one in front where they meet, and the one
        // the person is still talking to.
        // SAFETY: plain calls on a window handle.
        unsafe { SetWindowPos(syn, HWND_TOP, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOMOVE) };
        shared::raise(syn);
        Ok(describe(req, beside.as_ref().map(|(_, who)| who.as_str()), crowded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect { left: 0, top: 0, right: 1920, bottom: 1032 };

    fn req(body: &str) -> Result<Request, String> {
        parse(body)
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
    fn a_request_names_a_side_and_a_share_and_nothing_else() {
        assert_eq!(req(r#"{"side":"right","share":0.33}"#).unwrap(), Request { side: Side::Right, share: 0.33 });
        assert_eq!(req(r#"{"share":0.5,"side":"left"}"#).unwrap().side, Side::Left);
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
            r#"[1]"#,
            "not json",
            "",
        ] {
            let e = req(bad).unwrap_err();
            // Each refusal says what a good call looks like.
            assert!(e.contains(r#"{"side":"left"|"right","share":0.2 to 0.8}"#), "{bad}: {e}");
        }
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
        assert!(apply(Request { side: Side::Right, share: 0.33 }).unwrap_err().contains("only done on Windows"));
    }
}
