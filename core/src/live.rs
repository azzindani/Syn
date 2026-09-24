//! A run's progress, on disk, so something other than the process making it
//! can watch.
//!
//! The console used to show only what its own child printed. A run started
//! from a terminal, a pipe or a second window was invisible to it -- not
//! because the information was missing but because it never left the
//! process that had it. Every CLI now mirrors its output to
//! `.agent/live/<pid>.log`, and anything that wants to watch tails the
//! newest one.
//!
//! Best-effort throughout: a run must not fail because its progress could
//! not be written down.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::chats::home;

/// Logs older than this are from runs that are over, and are swept on start.
const STALE: Duration = Duration::from_secs(6 * 60 * 60);

fn dir() -> PathBuf {
    home().join("live")
}

/// This process's log. Held open: reopening per line turns a progress
/// mirror into a syscall storm on a run that prints hundreds of them.
static SINK: Mutex<Option<File>> = Mutex::new(None);

/// Begin mirroring. Called once at startup; failure is not fatal and not
/// reported, because a console that cannot watch is better than a run that
/// will not start.
pub fn begin() {
    let d = dir();
    if fs::create_dir_all(&d).is_err() {
        return;
    }
    sweep(&d);
    let path = d.join(format!("{}.log", std::process::id()));
    if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path)
        && let Ok(mut g) = SINK.lock()
    {
        *g = Some(f);
    }
}

/// This log is a record of work happening, not of everything the console
/// says. The page asks its child for the chat list and a transcript every
/// time it loads, and mirroring those answers put two hundred lines of
/// `MSG {...}` JSON into the progress strip and kept the file's mtime
/// fresh forever, so a run looked permanently in progress.
///
/// Progress is what a person watching would want to see: the steps, the
/// refusals, the answer, and the receipts of real operations.
fn is_progress(line: &str) -> bool {
    // Bulk answers to the page's own queries, not work.
    // `LABEL ` is one per tool call in a reopened transcript -- the page's
    // own query again, and on a long conversation it is hundreds of lines
    // that would keep the log's mtime fresh and make a finished run look
    // permanently in progress.
    if line.starts_with("MSG ")
        || line.starts_with("CHAT ")
        || line.starts_with("SLOT ")
        || line.starts_with("WIRE ")
        || line.starts_with("LABEL ")
    {
        return false;
    }
    // The console's own framing sentinel, one per request.
    if line.starts_with("RECEIPT mark=") {
        return false;
    }
    !line.trim().is_empty()
}

/// Mirror one line. Flushed immediately: a watcher reading a half-written
/// buffer is the whole failure mode this is meant to avoid.
pub fn append(line: &str) {
    if !is_progress(line) {
        return;
    }
    let Ok(mut g) = SINK.lock() else { return };
    let Some(f) = g.as_mut() else { return };
    let _ = writeln!(f, "{line}");
    let _ = f.flush();
}

/// Remove logs left by runs that are long over. Without this the directory
/// grows forever and `newest` has more to sort through every time.
fn sweep(d: &Path) {
    let Ok(rd) = fs::read_dir(d) else { return };
    let now = SystemTime::now();
    for e in rd.flatten() {
        let Ok(m) = e.metadata() else { continue };
        let Ok(t) = m.modified() else { continue };
        if now.duration_since(t).unwrap_or_default() > STALE {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// A CLI prints this many lines just by starting up -- the env receipt and
/// the banner drain. A log that has never got past them belongs to a
/// process that has not been asked to do anything.
const BANNER_LINES: usize = 3;

/// The run worth watching: the most recently written log that belongs to a
/// process actually doing something.
///
/// Most-recent alone is not enough. A run waits on the provider between
/// steps and is silent for up to a minute at a time, while a console that
/// has just spawned its child writes a banner and nothing else. On mtime
/// the idle newcomer beat the working run, and the page followed it into
/// an empty file. A log still on its banner is not a run.
pub fn newest() -> Option<PathBuf> {
    newest_in(&dir())
}

fn newest_in(d: &Path) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for e in fs::read_dir(d).ok()?.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("log") {
            continue;
        }
        let Ok(t) = e.metadata().and_then(|m| m.modified()) else { continue };
        if !past_banner(&p) {
            continue;
        }
        if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
            best = Some((t, p));
        }
    }
    best.map(|(_, p)| p)
}

/// The log a process with this pid writes.
pub fn log_of(pid: u32) -> PathBuf {
    dir().join(format!("{pid}.log"))
}

/// A log named by a page, if it is one of ours: `<digits>.log`, in the
/// live directory. Anything else is refused, so a query string cannot
/// point the console at another file on the machine.
pub fn named(name: &str) -> Option<PathBuf> {
    let stem = name.strip_suffix(".log")?;
    (!stem.is_empty() && stem.chars().all(|c| c.is_ascii_digit())).then(|| dir().join(name))
}

/// Which run the console should show, given the one it is showing now.
///
/// It used to be whichever log was written last. With two runs going at
/// once -- the console's own turn and an MCP client, or two MCP clients --
/// that flipped on every step, and each flip wiped the view and replayed
/// the other run from the top. Now: the console's own run while it is
/// mid-turn, because that is the one the person just asked for; otherwise
/// stay with the current run while it is still going; only then move to
/// the newest.
pub fn follow(current: Option<&Path>, own: Option<&Path>, own_busy: bool) -> Option<PathBuf> {
    follow_in(&dir(), current, own, own_busy)
}

fn follow_in(d: &Path, current: Option<&Path>, own: Option<&Path>, own_busy: bool) -> Option<PathBuf> {
    if own_busy
        && let Some(o) = own.filter(|o| past_banner(o))
    {
        return Some(o.to_path_buf());
    }
    // Never sticky on the console's own log when it is not mid-turn: the
    // page's own queries (`slots`, `wiring`, `chat list`) keep it fresh,
    // and holding on to it would hide a run another client just started.
    let sticky = current.filter(|c| Some(*c) != own && written_within(c, STICKY) && past_banner(c));
    if let Some(c) = sticky {
        return Some(c.to_path_buf());
    }
    newest_in(d)
}

/// How long a run keeps the view after its last line while another run is
/// writing. Longer than a step's usual pause, so two runs taking turns do
/// not flip it every step; short enough that a run which has finished
/// hands over within the minute rather than after `QUIET`.
const STICKY: Duration = Duration::from_secs(45);

fn written_within(p: &Path, d: Duration) -> bool {
    let Ok(t) = fs::metadata(p).and_then(|m| m.modified()) else { return false };
    SystemTime::now().duration_since(t).unwrap_or(Duration::MAX) < d
}

/// Whether a log has got past the startup banner. Only the first few lines
/// are read: this runs for every log on every look, and it used to read
/// each one whole -- which, once a streamed reply puts every few words of
/// the answer in the log, is megabytes a second for a question about four
/// lines.
fn past_banner(p: &Path) -> bool {
    use std::io::{BufRead, BufReader, Read};
    let Ok(f) = File::open(p) else { return false };
    let mut r = BufReader::new(f.take(64 * 1024));
    let mut buf = Vec::new();
    for _ in 0..=BANNER_LINES {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return false,
            Ok(_) => {}
        }
    }
    true
}

/// A place in a log, kept as a byte offset, so following a run reads only
/// what was added. The stream used to re-read the whole file every 60ms.
/// The line count is still what a page holds (`since`, `n`), so the two
/// transports and a reconnect all speak the same cursor.
#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    offset: u64,
    lines: usize,
}

/// The most one read hands over. A page that reconnects to a long run gets
/// the rest on the next read, 60ms later, rather than one enormous frame.
const READ_MAX: u64 = 1 << 20;

impl Tail {
    /// Placed after the first `since` lines (or at the end, if there are
    /// fewer).
    pub fn at(path: &Path, since: usize) -> Self {
        use std::io::{BufRead, BufReader};
        let mut t = Tail { path: path.to_path_buf(), offset: 0, lines: 0 };
        let Ok(f) = File::open(path) else { return t };
        let mut r = BufReader::new(f);
        let mut buf = Vec::new();
        while t.lines < since {
            buf.clear();
            match r.read_until(b'\n', &mut buf) {
                Ok(n) if n > 0 && buf.last() == Some(&b'\n') => {
                    t.offset += n as u64;
                    t.lines += 1;
                }
                _ => break,
            }
        }
        t
    }

    /// Lines counted so far.
    pub fn n(&self) -> usize {
        self.lines
    }

    /// The whole lines written since the last read. A line still being
    /// written (no newline yet) waits for the next read, so it is never
    /// handed over in two halves.
    pub fn read(&mut self) -> Vec<String> {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = File::open(&self.path) else { return Vec::new() };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        // Shorter than where we are: the file was replaced. Start over
        // rather than read from the middle of something else.
        if len < self.offset {
            self.offset = 0;
            self.lines = 0;
        }
        if len == self.offset || f.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut buf = Vec::new();
        if f.take(READ_MAX).read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { return Vec::new() };
        let text = String::from_utf8_lossy(&buf[..end]);
        let out: Vec<String> = text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
        self.offset += end as u64 + 1;
        self.lines += out.len();
        out
    }
}

/// Lines after `since`, and the new count.
pub fn read_from(path: &Path, since: usize) -> (usize, Vec<String>) {
    // An unreadable log leaves the cursor where it was: the page keeps its
    // place and picks up when the file is back.
    if !path.is_file() {
        return (since, Vec::new());
    }
    let mut t = Tail::at(path, since);
    let lines = t.read();
    (t.n(), lines)
}

/// Whether the run behind this log is still going. A process that has
/// stopped printing looks the same as one that has exited, which is the
/// right answer for a progress strip either way.
///
/// The window is generous on purpose. A turn waits on the provider between
/// steps, and a model that thinks without streaming its reasoning is
/// silent until it answers -- up to the stream's idle limit (180s by
/// default). Twenty seconds called it finished while it was still thinking.
const QUIET: Duration = Duration::from_secs(200);

pub fn active(path: &Path) -> bool {
    let Ok(t) = fs::metadata(path).and_then(|m| m.modified()) else { return false };
    SystemTime::now().duration_since(t).unwrap_or(Duration::MAX) < QUIET
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_runs_at_once_do_not_steal_the_view_from_each_other() {
        let d = std::env::temp_dir().join(format!("agent-live-follow-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let (a, b, own) = (d.join("100.log"), d.join("200.log"), d.join("300.log"));
        let body = "a\nb\nc\nd\ne\n";
        fs::write(&a, body).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(&b, body).unwrap();
        // B wrote last, but the view is on A and A is still going: stay.
        assert_eq!(follow_in(&d, Some(&a), None, false), Some(a.clone()));
        // Nothing followed yet: the newest.
        assert_eq!(follow_in(&d, None, None, false), Some(b.clone()));
        // The console's own turn wins while it is running...
        fs::write(&own, body).unwrap();
        assert_eq!(follow_in(&d, Some(&a), Some(&own), true), Some(own.clone()));
        // ...but not a child that has only printed its banner.
        fs::write(&own, "env\nbanner\n").unwrap();
        assert_eq!(follow_in(&d, Some(&a), Some(&own), true), Some(a.clone()));
        // And an idle console's own log never holds the view: the page's
        // queries keep it fresh, and another client's run must show.
        fs::write(&own, body).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(&b, body).unwrap();
        assert_eq!(follow_in(&d, Some(&own), Some(&own), false), Some(b.clone()));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_page_can_only_name_a_run_log() {
        assert!(named("1234.log").is_some());
        for bad in ["../x.log", "1234", ".log", "12a.log", "/etc/passwd", "12.log/..", ""] {
            assert!(named(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn reading_from_a_cursor_returns_only_what_is_new() {
        let d = std::env::temp_dir().join(format!("agent-live-{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        let p = d.join("t.log");
        fs::write(&p, "one\ntwo\nthree\n").unwrap();
        let (n, lines) = read_from(&p, 0);
        assert_eq!(n, 3);
        assert_eq!(lines, vec!["one", "two", "three"]);
        let (n2, lines2) = read_from(&p, 3);
        assert_eq!(n2, 3);
        assert!(lines2.is_empty(), "a cursor at the end must return nothing");
        fs::write(&p, "one\ntwo\nthree\nfour\n").unwrap();
        let (n3, lines3) = read_from(&p, 3);
        assert_eq!(n3, 4);
        assert_eq!(lines3, vec!["four"]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_cursor_past_the_end_never_panics() {
        let d = std::env::temp_dir().join(format!("agent-live-b-{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        let p = d.join("t.log");
        fs::write(&p, "only\n").unwrap();
        // A watcher that held a cursor into a longer, since-replaced file
        // must get an empty answer rather than an out-of-range slice.
        let (n, lines) = read_from(&p, 99);
        assert_eq!(n, 1);
        assert!(lines.is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_console_that_has_only_just_started_does_not_outrank_a_working_run() {
        let d = std::env::temp_dir().join(format!("agent-live-c-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        // A run mid-job, silent for a moment while it waits on the provider.
        let run = d.join("100.log");
        fs::write(&run, "RECEIPT env
RECEIPT hand
STEP read: a
STEP write: b
STEP write: c
").unwrap();
        // A console child that has just spawned and done nothing since.
        let idle = d.join("200.log");
        fs::write(&idle, "RECEIPT env
RECEIPT mark=m1
").unwrap();
        let mut best: Option<(SystemTime, PathBuf)> = None;
        for e in fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            let t = e.metadata().unwrap().modified().unwrap();
            if !past_banner(&p) {
                continue;
            }
            if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
                best = Some((t, p));
            }
        }
        assert_eq!(
            best.map(|(_, p)| p),
            Some(run),
            "a log still on its startup banner must not win over a run doing work"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_followed_log_hands_over_only_whole_new_lines() {
        let d = std::env::temp_dir().join(format!("agent-live-tail-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let p = d.join("100.log");
        fs::write(&p, "one\ntwo\nthr").unwrap();
        // Placed after the first line, the way a page resumes.
        let mut t = Tail::at(&p, 1);
        assert_eq!(t.n(), 1);
        assert_eq!(t.read(), vec!["two".to_string()], "a half-written line waits");
        assert!(t.read().is_empty());
        OpenOptions::new().append(true).open(&p).unwrap().write_all(b"ee\nfour\n").unwrap();
        assert_eq!(t.read(), vec!["three".to_string(), "four".to_string()]);
        assert_eq!(t.n(), 4);
        // Replaced by something shorter: start again, never mid-file.
        fs::write(&p, "new\n").unwrap();
        assert_eq!(t.read(), vec!["new".to_string()]);
        assert_eq!(t.n(), 1);
        // A cursor past the end sits at the end.
        assert_eq!(read_from(&p, 9), (1, Vec::new()));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn page_chatter_is_not_progress() {
        // Loading the console asks its child for the slot table, the wiring,
        // the chat list and a whole transcript. None of that is a run, and
        // mirroring it drowned the progress strip and left the log's mtime
        // permanently fresh, so every console looked busy.
        for noise in [
            r#"MSG {"role":"tool","id":"call-1","text":"..."}"#,
            r#"CHAT {"id":"c1789831309-7488"}"#,
            r#"SLOT {"slot":"Small","task":"skim"}"#,
            r#"WIRE excel -> hand-excel"#,
            "RECEIPT mark=m23",
            "   ",
        ] {
            assert!(!is_progress(noise), "{noise:?} is not work happening");
        }
        for real in [
            "STEP write: filled data!I2:I258424 (258,423 cells) from =YEAR(D2)",
            "REFUSED shell: \"python3\" is not on the allowlist",
            "STOPPED the model ended the turn with no answer and no tool call",
            "ANSWER I built the scorecard and the dashboard.",
            "ERROR live app refused the op",
            "RECEIPT say model=x open=3",
        ] {
            assert!(is_progress(real), "{real:?} is exactly what a watcher wants to see");
        }
    }

    #[test]
    fn a_missing_log_reads_as_empty_rather_than_failing() {
        let (n, lines) = read_from(Path::new("no/such/file.log"), 7);
        assert_eq!(n, 7, "an unreadable log must leave the cursor where it was");
        assert!(lines.is_empty());
    }
}
