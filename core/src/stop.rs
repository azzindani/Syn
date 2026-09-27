//! Stopping a turn that is already running.
//!
//! The console drives its CLI one command at a time over stdin, and a turn
//! holds that line for as long as it runs. "Stop everything" was a command
//! like any other, so it queued behind the turn it was meant to stop: a run
//! stuck on Excel recalculating a quadratic formula left the page with no
//! way out but closing it. A stop has to arrive by another road.
//!
//! That road is a file. The console writes `stop/<pid>` beside the chats;
//! the CLI with that pid looks for it between steps, while it waits to
//! retry, and while a reply streams in, and ends the turn cleanly when it
//! is there. Where the CLI cannot look -- inside a call to an application
//! that is not answering -- the console gives it a few seconds and then
//! restarts it, which is the part a file cannot do.

use std::path::PathBuf;

/// Where a stop for the process `pid` is asked for.
pub fn flag_for(pid: u32) -> PathBuf {
    crate::chats::home().join("stop").join(pid.to_string())
}

/// Ask the CLI with this pid to stop its turn.
pub fn request(pid: u32) -> std::io::Result<()> {
    let p = flag_for(pid);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(p, b"stop")
}

/// Whether this process has been asked to stop its turn.
pub fn requested() -> bool {
    flag_for(std::process::id()).exists()
}

/// Forget a stop: at the start of every turn, so one pressed after a run
/// finished cannot cut off the next, and once it has been honoured.
pub fn clear() {
    let _ = std::fs::remove_file(flag_for(std::process::id()));
}

/// Sleep, waking early if a stop arrives. Returns false when it did.
pub fn sleep(total: std::time::Duration) -> bool {
    let until = std::time::Instant::now() + total;
    while std::time::Instant::now() < until {
        if requested() {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100).min(until - std::time::Instant::now()));
    }
    !requested()
}

/// What a turn ended by a stop says, in the transcript and to the page.
pub const STOPPED: &str = "you pressed Stop";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stop_is_seen_by_this_process_and_cleared_after() {
        // Only this process's own flag: a stop for another CLI is not ours.
        let _ = std::fs::remove_file(flag_for(std::process::id()));
        assert!(!requested());
        request(std::process::id() + 1_000_000).unwrap();
        assert!(!requested(), "another process's stop is not this one's");
        let _ = std::fs::remove_file(flag_for(std::process::id() + 1_000_000));
        request(std::process::id()).unwrap();
        assert!(requested());
        assert!(!sleep(std::time::Duration::from_secs(5)), "a sleep wakes on a stop instead of running out");
        clear();
        assert!(!requested());
    }
}
