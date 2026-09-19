//! Runner: binds the run queue to op dispatch.
//! pump() executes the next queued op through ops::execute; a DoomLoop error
//! auto-pauses the run for human review. resume() revalidates queued handles
//! against the live registry first (files may have closed mid-pause).
//!
//! A handle marked live dispatches through an attached `LiveHand` to a real
//! document instead of the in-memory model. It takes the SAME route to get
//! there — kill switch, app allowlist, doom-loop gate, registry check, event
//! feed — because an op that edits the document in front of the human wants
//! more supervision than one that edits a model in memory, not less.

use crate::bus::Relay;
use crate::guard::{Guard, app_of};
use crate::hand::LiveHand;
use crate::ops::{Call, OpOut, execute};
use crate::protocol::{Error, Result};
use crate::queue::{QueueState, QueuedOp, RunQueue};
use crate::security;
use std::collections::HashSet;

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
    hand: Option<Box<dyn LiveHand>>,
    live: HashSet<String>,
}

impl Runner {
    pub fn new(session: &str) -> Self {
        Self {
            session: session.into(),
            queue: RunQueue::new(),
            jobs: std::collections::HashMap::new(),
            seq: 0,
            guard: Guard::open(),
            hand: None,
            live: HashSet::new(),
        }
    }

    pub fn with_guard(session: &str, guard: Guard) -> Self {
        let mut r = Self::new(session);
        r.guard = guard;
        r
    }

    /// Bind a live hand. Nothing dispatches through it until a handle is
    /// marked live, so attaching one cannot change existing behaviour.
    pub fn attach_hand(&mut self, hand: Box<dyn LiveHand>) {
        self.hand = Some(hand);
    }

    pub fn has_hand(&self) -> bool {
        self.hand.is_some()
    }

    /// Route this handle to the live hand instead of the in-memory model.
    pub fn mark_live(&mut self, handle: &str) {
        self.live.insert(handle.to_string());
    }

    pub fn is_live(&self, handle: &str) -> bool {
        self.live.contains(handle) && self.hand.is_some()
    }

    /// Forget the hand and every live handle. Called when the pipe dies so
    /// the next op fails loudly instead of silently hitting the in-memory
    /// model and reporting success for a document it never touched.
    pub fn drop_hand(&mut self) {
        self.hand = None;
        self.live.clear();
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
        if self.is_live(&job.handle) {
            return self.pump_live(relay, job).map(Some);
        }
        match execute(relay, &self.session, &job.handle, job.call) {
            Ok(out) => Ok(Some(out)),
            Err(e) => {
                if matches!(e, Error::DoomLoop(_)) {
                    self.queue.freeze();
                }
                Err(e)
            }
        }
    }

    /// Dispatch one job to the live document behind the hand.
    ///
    /// No relay snapshot is taken: there is no in-memory content to copy, and
    /// undo for a live file belongs to the sidecar's `.bak` plus the app's
    /// own undo stack. Taking one here would record an empty state and make
    /// `undo` look available when it is not.
    fn pump_live(&mut self, relay: &mut Relay, job: Job) -> Result<OpOut> {
        let op = job.call.op();
        relay.emit(&self.session, "step.start", &job.handle, format!("{op:?} live"))?;
        if let Err(e) = relay.gate(&self.session, &format!("{op:?}"), &job.call.args_key()) {
            self.queue.freeze();
            return Err(e);
        }
        if !relay.registry(&self.session)?.contains(&job.handle) {
            return Err(Error::UnknownHandle(job.handle));
        }
        let hand = self.hand.as_mut().expect("is_live() proved a hand is attached");
        match hand.dispatch_call(&job.call, &job.handle) {
            Ok(reply) if reply.ok => {
                let preview = security::truncate_output(&reply.preview);
                relay.emit(&self.session, "step.done", &job.handle, format!("{preview} live"))?;
                Ok(OpOut::Text { detail: reply.preview })
            }
            Ok(reply) => {
                let err = security::truncate_output(&reply.error);
                relay.emit(&self.session, "step.error", &job.handle, err)?;
                Err(Error::Live(reply.error))
            }
            Err(e) => {
                // The pipe is gone. Pause and forget the hand: continuing
                // would quietly fall back to the in-memory model.
                self.queue.freeze();
                self.drop_hand();
                relay.emit(&self.session, "step.error", &job.handle, format!("transport {e}"))?;
                Err(Error::Transport(e.to_string()))
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

    /// Scripted hand: hands back canned replies and records what it was
    /// asked, so the live path can be tested with no Office and no pipe.
    #[derive(Debug, Default)]
    struct FakeHand {
        replies: std::collections::VecDeque<std::io::Result<crate::hand::Reply>>,
        seen: Vec<String>,
    }

    impl FakeHand {
        fn ok(previews: &[&str]) -> Self {
            Self {
                replies: previews
                    .iter()
                    .map(|p| Ok(crate::hand::Reply { ok: true, preview: (*p).into(), error: String::new() }))
                    .collect(),
                seen: Vec::new(),
            }
        }
    }

    impl LiveHand for FakeHand {
        fn dispatch_call(&mut self, call: &Call, handle: &str) -> std::io::Result<crate::hand::Reply> {
            self.seen.push(format!("{:?} {handle}", call.op()));
            self.replies.pop_front().unwrap_or_else(|| {
                Ok(crate::hand::Reply { ok: true, preview: "default".into(), error: String::new() })
            })
        }
    }

    fn evs(r: &Relay, s: &str) -> Vec<String> {
        r.events(s).unwrap().into_iter().map(|e| e.t).collect()
    }

    #[test]
    fn live_handle_goes_to_the_hand_not_the_model() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.attach_hand(Box::new(FakeHand::ok(&["grid Sheet1: 5x3"])));
        run.mark_live(&h);
        run.submit(read_job(&h));
        let out = run.pump(&mut r).unwrap().unwrap();
        assert_eq!(out, OpOut::Text { detail: "grid Sheet1: 5x3".into() });
        let kinds = evs(&r, &s);
        assert!(kinds.contains(&"step.start".to_string()));
        assert!(kinds.contains(&"step.done".to_string()));
        // No snapshot was taken: undo for a live file is the sidecar's job.
        assert_eq!(r.snapshot_len(&s, &h), 0);
    }

    #[test]
    fn attaching_a_hand_alone_changes_nothing() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.attach_hand(Box::new(FakeHand::ok(&["should not be used"])));
        run.submit(read_job(&h)); // handle NOT marked live
        let out = run.pump(&mut r).unwrap().unwrap();
        assert!(matches!(out, OpOut::Grid { .. }), "in-memory path must still run, got {out:?}");
        assert!(!run.is_live(&h));
    }

    #[test]
    fn kill_switch_stops_live_dispatch_too() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.attach_hand(Box::new(FakeHand::ok(&["never"])));
        run.mark_live(&h);
        run.submit(read_job(&h));
        run.kill();
        assert!(matches!(run.pump(&mut r), Err(Error::Killed)));
    }

    #[test]
    fn allowlist_blocks_live_dispatch_too() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.attach_hand(Box::new(FakeHand::ok(&["never"])));
        run.mark_live(&h);
        run.lock_allowlist(vec!["word".into()]); // handle is excel:
        run.submit(read_job(&h));
        assert!(matches!(run.pump(&mut r), Err(Error::AppDenied(_))));
    }

    #[test]
    fn doom_loop_gate_applies_on_the_live_path() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.attach_hand(Box::new(FakeHand::ok(&["a", "b", "c"])));
        run.mark_live(&h);
        for _ in 0..3 {
            run.submit(read_job(&h));
        }
        assert!(run.pump(&mut r).is_ok());
        assert!(run.pump(&mut r).is_ok());
        assert!(matches!(run.pump(&mut r), Err(Error::DoomLoop(_))));
        assert_eq!(run.state(), &QueueState::Paused, "a doom loop must freeze the run");
    }

    #[test]
    fn unknown_handle_never_reaches_the_hand() {
        let (mut r, s, _h) = relay1();
        let mut run = Runner::new(&s);
        run.attach_hand(Box::new(FakeHand::ok(&["never"])));
        run.mark_live("excel:ghost.xlsx:Sheet1");
        run.submit(read_job("excel:ghost.xlsx:Sheet1"));
        assert!(matches!(run.pump(&mut r), Err(Error::UnknownHandle(_))));
    }

    #[test]
    fn app_level_refusal_becomes_a_live_error_with_an_event() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        let mut fake = FakeHand::default();
        fake.replies.push_back(Ok(crate::hand::Reply {
            ok: false,
            preview: String::new(),
            error: "workbook not open for excel:p.xlsx:Sheet1".into(),
        }));
        run.attach_hand(Box::new(fake));
        run.mark_live(&h);
        run.submit(read_job(&h));
        match run.pump(&mut r) {
            Err(Error::Live(d)) => assert!(d.contains("workbook not open")),
            other => panic!("expected Live, got {other:?}"),
        }
        assert!(evs(&r, &s).contains(&"step.error".to_string()));
        assert!(run.has_hand(), "an app-level refusal must not drop the connection");
    }

    #[test]
    fn dead_pipe_drops_the_hand_and_freezes() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        let mut fake = FakeHand::default();
        fake.replies.push_back(Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "sidecar closed the pipe",
        )));
        run.attach_hand(Box::new(fake));
        run.mark_live(&h);
        run.submit(read_job(&h));
        run.submit(read_job(&h));
        assert!(matches!(run.pump(&mut r), Err(Error::Transport(_))));
        assert_eq!(run.state(), &QueueState::Paused);
        // Critical: the next op must NOT silently succeed against the
        // in-memory model and report a change to a document nobody touched.
        assert!(!run.has_hand());
        assert!(!run.is_live(&h));
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
        assert!(matches!(run.pump(&mut r), Err(crate::protocol::Error::AppDenied(_))));
    }

    #[test]
    fn kill_stops_dispatch_and_pauses() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.submit(read_job(&h));
        run.submit(read_job(&h));
        run.kill();
        assert_eq!(run.state(), &QueueState::Paused);
        assert!(matches!(run.pump(&mut r), Err(crate::protocol::Error::Killed)));
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
