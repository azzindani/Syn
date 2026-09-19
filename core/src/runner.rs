//! Runner: binds the run queue to op dispatch.
//! pump() executes the next queued op through ops::execute; a DoomLoop error
//! auto-pauses the run for human review. resume() revalidates queued handles
//! against the live registry first (files may have closed mid-pause).

use crate::bus::Relay;
use crate::guard::{Guard, app_of};
use crate::ops::{Call, OpOut, execute};
use crate::protocol::Result;
use crate::queue::{QueueState, QueuedOp, RunQueue};

#[derive(Debug, Clone)]
pub struct Job {
    pub handle: String,
    pub summary: String,
    pub call: Call,
}

#[derive(Debug, Default)]
pub struct Runner {
    session: String,
    queue: RunQueue,
    jobs: std::collections::HashMap<String, Job>,
    seq: u64,
    guard: Guard,
}

impl Runner {
    pub fn new(session: &str) -> Self {
        Self { session: session.into(), queue: RunQueue::new(), jobs: std::collections::HashMap::new(), seq: 0, guard: Guard::open() }
    }

    pub fn with_guard(session: &str, guard: Guard) -> Self {
        let mut r = Self::new(session);
        r.guard = guard;
        r
    }

    /// Latch the kill switch: in-flight dispatch stops here.
    pub fn kill(&mut self) {
        self.guard.kill();
        self.queue.pause();
    }

    /// Lock dispatch to the named apps (CLI `allow` command).
    pub fn lock_allowlist(&mut self, apps: Vec<String>) {
        let killed = self.guard.is_killed();
        self.guard = Guard::allowlist(apps);
        if killed {
            self.guard.kill();
        }
    }

    pub fn state(&self) -> &QueueState {
        self.queue.state()
    }

    pub fn pending(&self) -> usize {
        self.queue.pending()
    }

    pub fn submit(&mut self, job: Job) -> String {
        self.seq += 1;
        let id = format!("job{:04}", self.seq);
        self.jobs.insert(id.clone(), job.clone());
        self.queue.enqueue(QueuedOp { handle: job.handle, summary: format!("{id}: {}", job.summary) });
        id
    }

    pub fn pause(&mut self) {
        self.queue.pause();
    }

    pub fn cancel(&mut self) {
        self.queue.cancel();
        self.jobs.clear();
    }

    /// Resume after revalidating handles. Returns vanished handles.
    pub fn resume(&mut self, relay: &Relay) -> Result<Vec<String>> {
        let registry = relay.registry(&self.session)?;
        let missing = self.queue.revalidate(&registry);
        self.queue.resume();
        Ok(missing)
    }

    /// Execute the next queued job. DoomLoop auto-pauses the run.
    pub fn pump(&mut self, relay: &mut Relay) -> Result<Option<OpOut>> {
        self.guard.armed()?;
        let Some(next) = self.queue.pop_next() else {
            return Ok(None);
        };
        let id = next.summary.split(':').next().unwrap_or("").to_string();
        let Some(job) = self.jobs.remove(&id) else {
            return Ok(None);
        };
        self.guard.check(app_of(&job.handle))?;
        match execute(relay, &self.session, &job.handle, job.call) {
            Ok(out) => Ok(Some(out)),
            Err(e) => {
                if matches!(e, crate::protocol::HarnessError::DoomLoop(_)) {
                    self.queue.freeze();
                }
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{FileContent, FileKind, OpenFile};
    use crate::ops::ReadArgs;
    use std::collections::HashMap;

    fn relay1() -> (Relay, String, String) {
        let mut r = Relay::new();
        let s = "s".to_string();
        r.handshake(&s, "t");
        let h = crate::protocol::new_handle("excel", "p.xlsx", "Sheet1");
        r.attach(&s, h.clone(), OpenFile {
            kind: FileKind::Excel,
            content: FileContent::Excel { sheets: HashMap::from([("Sheet1".into(), vec![vec!["1".into()]])]) },
            styles: HashMap::new(),
        });
        (r, s, h)
    }

    fn read_job(h: &str) -> Job {
        Job { handle: h.into(), summary: "read".into(), call: Call::Read(ReadArgs { selector: "Sheet1".into() }) }
    }

    #[test]
    fn pump_executes_and_drains() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.submit(read_job(&h));
        assert!(run.pump(&mut r).unwrap().is_some());
        assert!(run.pump(&mut r).unwrap().is_none());
        assert_eq!(run.state(), &QueueState::Idle);
    }

    #[test]
    fn pause_blocks_pump() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.submit(read_job(&h));
        run.pause();
        assert!(run.pump(&mut r).unwrap().is_none());
        run.resume(&r).unwrap();
        assert!(run.pump(&mut r).unwrap().is_some());
    }

    #[test]
    fn resume_prunes_vanished_files() {
        let (r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.submit(read_job(&h));
        run.submit(Job { handle: "excel:gone.xlsx:S1".into(), summary: "read".into(), call: Call::Read(ReadArgs { selector: "S1".into() }) });
        run.pause();
        let missing = run.resume(&r).unwrap();
        assert_eq!(missing, vec!["excel:gone.xlsx:S1".to_string()]);
        assert_eq!(run.pending(), 1);
    }

    #[test]
    fn doomloop_autopauses() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        for _ in 0..3 {
            run.submit(read_job(&h));
        }
        assert!(run.pump(&mut r).unwrap().is_some());
        assert!(run.pump(&mut r).unwrap().is_some());
        assert!(run.pump(&mut r).is_err());
        assert_eq!(run.state(), &QueueState::Paused);
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;
    use crate::bus::{FileContent, FileKind, OpenFile};
    use crate::guard::Guard;
    use crate::ops::ReadArgs;
    use std::collections::HashMap;

    fn relay1() -> (Relay, String, String) {
        let mut r = Relay::new();
        let s = "s".to_string();
        r.handshake(&s, "t");
        let h = crate::protocol::new_handle("excel", "p.xlsx", "Sheet1");
        r.attach(&s, h.clone(), OpenFile {
            kind: FileKind::Excel,
            content: FileContent::Excel { sheets: HashMap::from([("Sheet1".into(), vec![vec!["1".into()]])]) },
            styles: HashMap::new(),
        });
        (r, s, h)
    }

    fn read_job(h: &str) -> Job {
        Job { handle: h.into(), summary: "read".into(), call: Call::Read(ReadArgs { selector: "Sheet1".into() }) }
    }

    #[test]
    fn guard_denies_unlisted_app_at_pump() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::with_guard(&s, Guard::locked(&["word"]));
        run.submit(read_job(&h)); // excel handle, word-only guard
        assert!(matches!(run.pump(&mut r), Err(crate::protocol::HarnessError::AppDenied(_))));
    }

    #[test]
    fn kill_stops_dispatch_and_pauses() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.submit(read_job(&h));
        run.submit(read_job(&h));
        run.kill();
        assert_eq!(run.state(), &QueueState::Paused);
        assert!(matches!(run.pump(&mut r), Err(crate::protocol::HarnessError::Killed)));
        assert_eq!(run.pending(), 2); // jobs retained for audit, never dispatched
    }

    #[test]
    fn cancel_clears_jobs() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.submit(read_job(&h));
        run.cancel();
        assert_eq!(run.pending(), 0);
        assert!(run.pump(&mut r).unwrap().is_none());
    }
}
