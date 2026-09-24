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
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for e in fs::read_dir(dir()).ok()?.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("log") {
            continue;
        }
        let Ok(t) = e.metadata().and_then(|m| m.modified()) else { continue };
        if lines_in(&p) <= BANNER_LINES {
            continue;
        }
        if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
            best = Some((t, p));
        }
    }
    best.map(|(_, p)| p)
}

fn lines_in(p: &Path) -> usize {
    fs::read_to_string(p).map(|t| t.lines().count()).unwrap_or(0)
}

/// Lines after `since`, and the new count. Reading the whole file each poll
/// is fine at these sizes and cannot miss a line the way a held offset can
/// when the file is replaced.
pub fn read_from(path: &Path, since: usize) -> (usize, Vec<String>) {
    let Ok(text) = fs::read_to_string(path) else { return (since, Vec::new()) };
    let all: Vec<&str> = text.lines().collect();
    let n = all.len();
    if since >= n {
        return (n, Vec::new());
    }
    (n, all[since..].iter().map(|s| s.to_string()).collect())
}

/// Whether the run behind this log is still going. A process that has
/// stopped printing looks the same as one that has exited, which is the
/// right answer for a progress strip either way.
///
/// The window is generous on purpose. A turn waits on the provider between
/// steps, and the request timeout alone is sixty seconds, so a live run is
/// routinely silent for a minute. Twenty seconds called it finished while
/// it was still thinking.
const QUIET: Duration = Duration::from_secs(150);

pub fn active(path: &Path) -> bool {
    let Ok(t) = fs::metadata(path).and_then(|m| m.modified()) else { return false };
    SystemTime::now().duration_since(t).unwrap_or(Duration::MAX) < QUIET
}

#[cfg(test)]
mod tests {
    use super::*;

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
            if lines_in(&p) <= BANNER_LINES {
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
