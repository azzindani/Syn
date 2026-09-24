//! Runner: binds the run queue to op dispatch.
//! pump() executes the next queued op through ops::execute; a DoomLoop error
//! auto-pauses the run for human review. resume() revalidates queued handles
//! against the live registry first (files may have closed mid-pause).
//!
//! A handle marked live dispatches through an attached `LiveHand` to a real
//! document instead of the in-memory model. Many hands can be attached at
//! once and a handle routes to the one that claims its app, so a single
//! session can hold Office, a browser and an editor at the same time and
//! the caller never picks a transport. It takes the SAME route to get
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
use std::collections::HashMap;

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
    hands: Vec<Attached>,
    /// handle -> name of the hand it is bound to.
    live: HashMap<String, String>,
}

/// One attached hand and the apps it claims.
///
/// An empty `apps` makes the hand a catch-all: it takes any handle no other
/// hand claims. A named claim beats a catch-all, and among equals the first
/// attached wins, so routing is insertion-ordered and never depends on hash
/// iteration order.
#[derive(Debug)]
struct Attached {
    name: String,
    apps: Vec<String>,
    hand: Box<dyn LiveHand>,
}

impl Runner {
    pub fn new(session: &str) -> Self {
        Self {
            session: session.into(),
            queue: RunQueue::new(),
            jobs: std::collections::HashMap::new(),
            seq: 0,
            guard: Guard::open(),
            hands: Vec::new(),
            live: HashMap::new(),
        }
    }

    pub fn with_guard(session: &str, guard: Guard) -> Self {
        let mut r = Self::new(session);
        r.guard = guard;
        r
    }

    /// Bind a hand under `name`, claiming `apps` (empty = any app).
    /// Nothing dispatches through it until a handle is marked live, so
    /// attaching one cannot change existing behaviour.
    ///
    /// Re-attaching a name replaces that hand and unlives its handles: a new
    /// pipe is a new process, and a document it never opened must be marked
    /// live again rather than inherited from its predecessor.
    pub fn attach_hand_as(&mut self, name: &str, apps: Vec<String>, hand: Box<dyn LiveHand>) {
        self.detach_hand(name);
        self.hands.push(Attached { name: name.into(), apps, hand });
    }

    /// Bind a catch-all hand named "default".
    pub fn attach_hand(&mut self, hand: Box<dyn LiveHand>) {
        self.attach_hand_as("default", Vec::new(), hand);
    }

    pub fn has_hand(&self) -> bool {
        !self.hands.is_empty()
    }

    /// Attached hands in routing order, as (name, claimed apps).
    pub fn hands(&self) -> Vec<(&str, &[String])> {
        self.hands.iter().map(|a| (a.name.as_str(), a.apps.as_slice())).collect()
    }

    /// The hand that would take this app, by the rule on `Attached`.
    fn route(&self, app: &str) -> Option<&str> {
        self.hands
            .iter()
            .find(|a| a.apps.iter().any(|x| x == app))
            .or_else(|| self.hands.iter().find(|a| a.apps.is_empty()))
            .map(|a| a.name.as_str())
    }

    /// Route this handle to a live hand instead of the in-memory model, and
    /// report which hand took it.
    ///
    /// Fails when no attached hand claims the app, so a handle for a hand
    /// that was never attached binds nothing rather than quietly staying on
    /// the in-memory model and reporting edits to a document no one touched.
    pub fn mark_live(&mut self, handle: &str) -> Result<String> {
        let app = app_of(handle).to_string();
        let Some(name) = self.route(&app).map(str::to_string) else {
            return Err(Error::NoHand(app));
        };
        self.live.insert(handle.to_string(), name.clone());
        Ok(name)
    }

    /// Ask the hand that claims this app to open a file.
    ///
    /// The one request that names no open document, because it is how a
    /// document becomes open. Without it a session could only ever drive
    /// what a human had already opened by hand, which is why the capability
    /// fixture needed a PowerShell script and somebody at the keyboard.
    pub fn open_file(&mut self, app: &str, path: &str) -> Result<String> {
        let Some(name) = self.route(app).map(str::to_string) else {
            return Err(Error::NoHand(app.to_string()));
        };
        let line = crate::hand::open_envelope(app, path);
        let Some(a) = self.hands.iter_mut().find(|a| a.name == name) else {
            return Err(Error::NoHand(app.to_string()));
        };
        match a.hand.send_envelope(&line) {
            Ok(reply) => reply.into_result().map_err(Error::Live),
            Err(e) => Err(Error::Transport(e.to_string())),
        }
    }

    pub fn is_live(&self, handle: &str) -> bool {
        self.hand_for(handle).is_some()
    }

    /// Name of the hand bound to this handle, if it is live and that hand is
    /// still attached.
    pub fn hand_for(&self, handle: &str) -> Option<&str> {
        let name = self.live.get(handle)?;
        self.hands.iter().find(|a| &a.name == name).map(|a| a.name.as_str())
    }

    /// Forget one hand and only the handles bound to it. Called when a pipe
    /// dies so the next op on those handles fails loudly instead of silently
    /// hitting the in-memory model and reporting success for a document it
    /// never touched. One dead transport must not unlive documents held by a
    /// hand that is still healthy.
    pub fn detach_hand(&mut self, name: &str) {
        self.hands.retain(|a| a.name != name);
        self.live.retain(|_, v| v != name);
    }

    /// Forget every hand and every live handle.
    pub fn drop_hand(&mut self) {
        self.hands.clear();
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
        // VBA is gated here, at the single point every op passes through,
        // rather than at the hand. A gate the live path alone enforces is
        // one an in-memory path can walk around, and this is the one
        // capability on the surface that executes code.
        if let crate::ops::Call::Struct(crate::ops::StructArgs::Macro { action, .. }) = &job.call
            && !crate::guard::vba_allowed()
        {
            return Err(Error::Denied(format!(
                "macro {action:?} refused: VBA is off. It runs code at your full privilege, \
                 so a human turns it on for a session with AGENT_VBA=1 and it is denied by \
                 default in protocol/security_policy.json"
            )));
        }
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
        let bound = self.live.get(&job.handle).cloned().expect("is_live() proved a hand is bound");
        let hand = self
            .hands
            .iter_mut()
            .find(|a| a.name == bound)
            .expect("hand_for() proved the bound hand is still attached");
        match hand.hand.dispatch_call(&job.call, &job.handle) {
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
                // The pipe is gone. Pause and forget THIS hand: continuing
                // would quietly fall back to the in-memory model. Hands on
                // other transports are untouched; they did not fail.
                self.queue.freeze();
                self.detach_hand(&bound);
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

    #[test]
    fn vba_is_refused_unless_a_human_switched_it_on() {
        // The default has to be off, and off at the dispatch every op
        // passes through -- not at the hand, which an in-memory path
        // would walk around. This is the one verb on the surface that
        // executes code, and it is strictly more powerful than `shell`,
        // which already stops for a human on every single call.
        let (mut relay, s, h) = relay1();
        let mut r = Runner::new(&s);
        r.submit(Job {
            handle: h.clone(),
            summary: "macro".into(),
            call: Call::Struct(crate::ops::StructArgs::Macro {
                action: "run".into(),
                module: "SynMacros".into(),
                code: String::new(),
                name: "DoThing".into(),
            }),
        });
        match r.pump(&mut relay) {
            Err(Error::Denied(why)) => {
                assert!(why.contains("VBA is off"), "{why}");
                assert!(why.contains("AGENT_VBA=1"), "a refusal must say how to allow it: {why}");
            }
            other => panic!("VBA must be denied by default, got {other:?}"),
        }
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
        run.mark_live(&h).unwrap();
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
        run.mark_live(&h).unwrap();
        run.submit(read_job(&h));
        run.kill();
        assert!(matches!(run.pump(&mut r), Err(Error::Killed)));
    }

    #[test]
    fn allowlist_blocks_live_dispatch_too() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.attach_hand(Box::new(FakeHand::ok(&["never"])));
        run.mark_live(&h).unwrap();
        run.lock_allowlist(vec!["word".into()]); // handle is excel:
        run.submit(read_job(&h));
        assert!(matches!(run.pump(&mut r), Err(Error::AppDenied(_))));
    }

    #[test]
    fn doom_loop_gate_applies_on_the_live_path() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.attach_hand(Box::new(FakeHand::ok(&["a", "b", "c"])));
        run.mark_live(&h).unwrap();
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
        run.mark_live("excel:ghost.xlsx:Sheet1").unwrap();
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
        run.mark_live(&h).unwrap();
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
        run.mark_live(&h).unwrap();
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

    /// Attach a word hand alongside the excel one and give the relay a word
    /// handle to route.
    fn with_word(r: &mut Relay, s: &str) -> String {
        let h = crate::protocol::new_handle("word", "d.docx", "body");
        r.attach(s, h.clone(), OpenFile {
            kind: FileKind::Word,
            content: FileContent::Word {
                paras: vec!["p".into()],
                tables: Vec::new(),
                comments: Vec::new(),
                changes: Vec::new(),
            },
            styles: HashMap::new(),
        });
        h
    }

    #[test]
    fn two_hands_each_get_their_own_app() {
        let (mut r, s, xh) = relay1();
        let wh = with_word(&mut r, &s);
        let mut run = Runner::new(&s);
        run.attach_hand_as("office", vec!["excel".into()], Box::new(FakeHand::ok(&["from excel hand"])));
        run.attach_hand_as("writer", vec!["word".into()], Box::new(FakeHand::ok(&["from word hand"])));
        assert_eq!(run.mark_live(&xh).unwrap(), "office");
        assert_eq!(run.mark_live(&wh).unwrap(), "writer");

        run.submit(read_job(&xh));
        let out = run.pump(&mut r).unwrap().unwrap();
        assert_eq!(out, OpOut::Text { detail: "from excel hand".into() });

        run.submit(Job {
            handle: wh.clone(),
            summary: "read".into(),
            call: Call::Read(ReadArgs { selector: "body".into() }),
        });
        let out = run.pump(&mut r).unwrap().unwrap();
        assert_eq!(out, OpOut::Text { detail: "from word hand".into() });
    }

    #[test]
    fn a_named_claim_beats_a_catch_all() {
        let (_r, s, xh) = relay1();
        let mut run = Runner::new(&s);
        run.attach_hand(Box::new(FakeHand::ok(&["catch-all"])));
        run.attach_hand_as("office", vec!["excel".into()], Box::new(FakeHand::ok(&["claimed"])));
        // Attached second, but it names the app, so it wins.
        assert_eq!(run.mark_live(&xh).unwrap(), "office");
        assert_eq!(run.mark_live("ppt:d.pptx:deck").unwrap(), "default");
    }

    #[test]
    fn marking_live_fails_when_no_hand_claims_the_app() {
        let (_r, s, xh) = relay1();
        let mut run = Runner::new(&s);
        run.attach_hand_as("writer", vec!["word".into()], Box::new(FakeHand::default()));
        // Fails closed: without this the handle would stay on the in-memory
        // model and report edits to a document nobody touched.
        assert!(matches!(run.mark_live(&xh), Err(Error::NoHand(a)) if a == "excel"));
        assert!(!run.is_live(&xh));
    }

    #[test]
    fn a_dead_pipe_drops_only_its_own_hand() {
        let (mut r, s, xh) = relay1();
        let wh = with_word(&mut r, &s);
        let mut run = Runner::new(&s);
        let dead = FakeHand {
            replies: std::collections::VecDeque::from([Err(std::io::Error::other("pipe closed"))]),
            seen: Vec::new(),
        };
        run.attach_hand_as("office", vec!["excel".into()], Box::new(dead));
        run.attach_hand_as("writer", vec!["word".into()], Box::new(FakeHand::ok(&["still here"])));
        run.mark_live(&xh).unwrap();
        run.mark_live(&wh).unwrap();

        run.submit(read_job(&xh));
        assert!(matches!(run.pump(&mut r), Err(Error::Transport(_))));
        // The excel hand is gone and its handle is no longer live...
        assert!(!run.is_live(&xh));
        assert_eq!(run.hands().len(), 1);
        // ...but the word hand never failed, so it keeps its document.
        assert!(run.is_live(&wh));
        assert_eq!(run.hand_for(&wh), Some("writer"));
    }

    #[test]
    fn reattaching_a_name_unlives_its_handles() {
        let (_r, s, xh) = relay1();
        let mut run = Runner::new(&s);
        run.attach_hand_as("office", vec!["excel".into()], Box::new(FakeHand::default()));
        run.mark_live(&xh).unwrap();
        // A new pipe is a new process: it never opened this document.
        run.attach_hand_as("office", vec!["excel".into()], Box::new(FakeHand::default()));
        assert!(!run.is_live(&xh));
        assert_eq!(run.hands().len(), 1);
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
