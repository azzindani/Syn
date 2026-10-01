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
    /// How a document that is not open yet becomes one, when this runner
    /// has a way. `None` in tests and in the in-memory REPL: `open` is then
    /// refused, the way it was before a model could ask for it.
    door: Option<Box<dyn Door>>,
    /// The folders documents may be written into: the console's workspace,
    /// MCP's AGENT_MCP_ROOTS. Empty is anywhere. Canonical.
    roots: Vec<std::path::PathBuf>,
    /// Files an `export` of this session wrote, which a later export may
    /// replace. Anything else already on disk is not Syn's to overwrite.
    written: std::collections::HashSet<std::path::PathBuf>,
}

/// The way from "a file on disk" to "a handle bound live", for a caller
/// that holds no desk of its own: the loop.
///
/// The console asked a person to connect a hand, attach a handle and bind
/// it live, three commands with an order to get right, before the model
/// could touch a file. A model asked to "open plan.xlsx" had no tool for
/// it, tried `shell excel.exe`, was refused, and told the person to do it
/// by hand. MCP already had one call that did the lot (`desk::Desk::open`);
/// this is the same door, fitted to the runner so the loop reaches it
/// without a second road to a document.
///
/// A door works on the runner it is fitted to, so it is lent the runner
/// and the relay rather than holding either. Everything it opens is bound
/// through `mark_live` and read through `run`, gates included.
pub trait Door: std::fmt::Debug {
    /// Open `target` in `app` (or find it already open), register it and
    /// bind it live. The document comes back whole rather than as a
    /// sentence: its summary was read out of the file and is untrusted,
    /// its handle and next step are ours, and the caller fences one and
    /// not the other.
    fn open(&mut self, relay: &mut Relay, runner: &mut Runner, app: &str, target: &str) -> std::result::Result<crate::desk::Doc, String>;
    /// Make a new, empty document at `target` and open it, as `open` does.
    /// A door that cannot make documents says so.
    fn create(&mut self, _relay: &mut Relay, _runner: &mut Runner, app: &str, _target: &str) -> std::result::Result<crate::desk::Doc, String> {
        Err(format!("this session cannot make a new {app} document"))
    }
    /// Files whose names match `query`, under `folder` or the usual places.
    fn find(&self, query: &str, folder: Option<&str>) -> std::result::Result<String, String>;
    /// Attach every wired helper that is already running, starting none.
    /// Returns the apps newly connected.
    fn connect_running(&mut self, relay: &mut Relay, runner: &mut Runner) -> Vec<String>;
    /// Keep `open` and `search` inside these folders; empty lifts the limit.
    /// The console's workspace, and MCP's AGENT_MCP_ROOTS, are this.
    fn confine(&mut self, _roots: Vec<std::path::PathBuf>) {}
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
            door: None,
            roots: Vec::new(),
            written: std::collections::HashSet::new(),
        }
    }

    pub fn with_guard(session: &str, guard: Guard) -> Self {
        let mut r = Self::new(session);
        r.guard = guard;
        r
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    /// Fit the door the loop's `open`, `search` and the console's
    /// `open` go through.
    pub fn set_door(&mut self, door: Box<dyn Door>) {
        self.door = Some(door);
    }

    pub fn has_door(&self) -> bool {
        self.door.is_some()
    }

    /// Confine the door, and every `export`, to these folders (empty:
    /// everywhere).
    pub fn confine(&mut self, roots: Vec<std::path::PathBuf>) {
        self.roots = roots.iter().map(|r| std::fs::canonicalize(r).unwrap_or_else(|_| r.clone())).collect();
        if let Some(d) = self.door.as_mut() {
            d.confine(roots);
        }
    }

    /// Where an `export` may write, as the full path to send, or why not.
    ///
    /// `export` writes a file wherever it is told and a helper's SaveCopyAs
    /// replaces whatever was there. Nothing held it to the workspace or to
    /// AGENT_MCP_ROOTS, which confined only `open`, so a document carrying
    /// "export this to C:\Users\you\Documents\thesis.docx" could have a
    /// model overwrite the person's own work with a copy of something else.
    /// Three rules: a relative path is under the workspace (or refused when
    /// there is none -- it would land wherever the helper was started); the
    /// path must be inside the roots; and a file already there is replaced
    /// only when this session's own export wrote it.
    fn export_target(&self, path: &str) -> std::result::Result<std::path::PathBuf, String> {
        use std::path::{Component, Path, PathBuf};
        let raw = Path::new(path.trim());
        let joined = if raw.is_absolute() || path.contains(':') {
            raw.to_path_buf()
        } else if let Some(root) = self.roots.first() {
            PathBuf::from(crate::find::display(root)).join(raw)
        } else {
            return Err(format!(
                "export: {path:?} is not a full path, and there is no workspace to put it under; give one, e.g. C:\\Users\\you\\Documents\\copy.xlsx"
            ));
        };
        // `..` taken out by hand: canonicalize needs the file to exist, and
        // an export's usually does not yet.
        let mut clean = PathBuf::new();
        for c in joined.components() {
            match c {
                Component::ParentDir => {
                    clean.pop();
                }
                Component::CurDir => {}
                other => clean.push(other),
            }
        }
        if !self.roots.is_empty() {
            // The deepest folder that exists, resolved, is what decides:
            // a link inside the workspace can still lead out of it.
            let mut probe = clean.clone();
            let mut rest = Vec::new();
            let resolved = loop {
                if let Ok(c) = std::fs::canonicalize(&probe) {
                    break rest.iter().rev().fold(c, |acc: PathBuf, part: &std::ffi::OsString| acc.join(part));
                }
                match (probe.file_name().map(|f| f.to_os_string()), probe.parent().map(Path::to_path_buf)) {
                    (Some(f), Some(p)) => {
                        rest.push(f);
                        probe = p;
                    }
                    _ => break clean.clone(),
                }
            };
            if !self.roots.iter().any(|r| resolved.starts_with(r)) {
                return Err(format!(
                    "export: {} is outside the folders Syn may write here ({}). In the console that is the chat's workspace; behind MCP it is AGENT_MCP_ROOTS.",
                    clean.display(),
                    self.roots.iter().map(|r| crate::find::display(r)).collect::<Vec<_>>().join(", ")
                ));
            }
        }
        if clean.is_file() && !self.written.contains(&clean) {
            return Err(format!(
                "export: {} already exists and this session did not write it. Export to a new name, or ask the human whether to replace it.",
                clean.display()
            ));
        }
        Ok(clean)
    }

    /// Lend the door this runner and the relay for one call.
    ///
    /// Taken out and put back rather than borrowed: the door needs the
    /// whole runner (to attach hands, bind handles and read through the
    /// gates), and it lives inside it.
    fn through_door<T>(
        &mut self,
        relay: &mut Relay,
        f: impl FnOnce(&mut dyn Door, &mut Relay, &mut Runner) -> std::result::Result<T, String>,
    ) -> std::result::Result<T, String> {
        let Some(mut door) = self.door.take() else {
            return Err("nothing here can open files: this session has no desk (start it from the console or cli, not a test harness)".into());
        };
        let out = f(door.as_mut(), relay, self);
        self.door = Some(door);
        out
    }

    /// Open a document through the door. The gates are checked here as
    /// well as inside, so a killed run or a disallowed app is refused
    /// before a helper is started for it.
    pub fn open_doc(&mut self, relay: &mut Relay, app: &str, target: &str) -> std::result::Result<crate::desk::Doc, String> {
        if let Some(key) = crate::desk::app_key(app) {
            self.permits(key).map_err(|e| e.to_string())?;
        }
        self.through_door(relay, |d, relay, runner| d.open(relay, runner, app, target))
    }

    /// Make a new, empty document through the door, then open it. The
    /// same gates as `open_doc`, before anything is started.
    pub fn create_doc(&mut self, relay: &mut Relay, app: &str, target: &str) -> std::result::Result<crate::desk::Doc, String> {
        if let Some(key) = crate::desk::app_key(app) {
            self.permits(key).map_err(|e| e.to_string())?;
        }
        self.through_door(relay, |d, relay, runner| d.create(relay, runner, app, target))
    }

    pub fn find_files(&mut self, query: &str, folder: Option<&str>) -> std::result::Result<String, String> {
        match &self.door {
            Some(d) => d.find(query, folder),
            None => Err("nothing here can search for files: this session has no desk".into()),
        }
    }

    /// Attach whatever helpers are already running. Cheap when they are
    /// not (a pipe or a port that is not there fails at once), so it is
    /// safe to call before every turn.
    pub fn connect_running(&mut self, relay: &mut Relay) -> Vec<String> {
        self.through_door(relay, |d, relay, runner| Ok(d.connect_running(relay, runner))).unwrap_or_default()
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

    /// Whether an attached hand would take this app's handles.
    pub fn serves(&self, app: &str) -> bool {
        self.route(app).is_some()
    }

    /// Would the gates let an op on this app through right now? Asked
    /// before anything expensive -- connecting a pipe, launching a helper,
    /// starting Excel -- so a refused app costs nothing to refuse.
    pub fn permits(&self, app: &str) -> Result<()> {
        self.guard.armed()?;
        self.guard.check(app)
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
        self.send_open(app, crate::hand::open_envelope(app, path))
    }

    /// Ask the hand that claims this app to make a new, empty document at
    /// `path`. Checked by the desk first: in the workspace, not over a file.
    pub fn create_file(&mut self, app: &str, path: &str) -> Result<String> {
        self.send_open(app, crate::hand::create_envelope(app, path))
    }

    fn send_open(&mut self, app: &str, line: String) -> Result<String> {
        let Some(name) = self.route(app).map(str::to_string) else {
            return Err(Error::NoHand(app.to_string()));
        };
        let Some(a) = self.hands.iter_mut().find(|a| a.name == name) else {
            return Err(Error::NoHand(app.to_string()));
        };
        match a.hand.send_envelope(&line) {
            Ok(reply) => reply.into_result().map_err(Error::Live),
            Err(e) => Err(Error::Transport(e.to_string())),
        }
    }

    /// The name of the hand that would serve `app`, if any.
    pub fn hand_serving(&self, app: &str) -> Option<String> {
        self.route(app).map(str::to_string)
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

    /// Run one op now, through every gate. The one road to a document.
    ///
    /// The agent loop, the REPL and the MCP server all call this, so there
    /// is no caller -- ours or someone else's model -- that reaches a
    /// document without meeting the kill switch, the app allowlist, the VBA
    /// gate, the doom-loop gate, the registry check and the event feed.
    /// `mcpgate` used to be the exception: its writes built an envelope
    /// and never met any of them (docs/design/split-plan.md section 4).
    ///
    /// `Ok(None)` means the queue is not running: paused, cancelled or
    /// frozen by the doom-loop gate.
    pub fn run(&mut self, relay: &mut Relay, handle: &str, summary: &str, call: Call) -> Result<Option<OpOut>> {
        // A paused run takes nothing new. This used to queue the job anyway
        // behind the pause; the first call after `resume` then ran it and
        // was handed its answer, and every read from there on came back one
        // behind -- asked for A1:D4, given the A1:C3 refused before. The
        // kill switch still answers as itself.
        if matches!(self.queue.state(), QueueState::Paused | QueueState::Cancelled) {
            self.guard.armed()?;
            return Ok(None);
        }
        self.submit(Job { handle: handle.into(), summary: summary.into(), call });
        self.pump(relay)
    }

    /// Thaw a run the gates froze, when the thing that froze it is dealt
    /// with: a human sent the next message after a repeated call, or a
    /// fresh hand replaced one whose pipe died. Nothing queued is carried
    /// over. A latched kill is not a pause and stays latched.
    pub fn thaw(&mut self, relay: &mut Relay) {
        if self.queue.state() == &QueueState::Paused {
            let _ = self.resume(relay);
            relay.forget_calls(&self.session);
        }
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
        // embedChart reads a chart out of a second document, named in its
        // `from`. That document gets the gates its own calls would: an app
        // the policy has off stays off, and a workbook the model was never
        // given is refused rather than found by name in whatever is open.
        if let crate::ops::Call::Struct(crate::ops::StructArgs::Office { verb, args, .. }) = &job.call
            && verb == "embedChart"
        {
            let from = args.iter().find(|(k, _)| k == "from").map(|(_, v)| v.as_str()).unwrap_or("");
            self.guard.check(app_of(from))?;
            if !relay.registry(&self.session)?.iter().any(|h| h == from) {
                return Err(Error::UnknownHandle(from.into()));
            }
        }
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
        // An export writes a file: held to the roots, and never over one
        // that is not Syn's (`export_target`). The path the helper is sent
        // is the one checked.
        let mut job = job;
        let mut writes = None;
        if let Call::Export(crate::ops::ExportArgs { path: Some(path), .. }) = &mut job.call
            && !path.trim().is_empty()
        {
            let target = self.export_target(path).map_err(Error::Denied)?;
            *path = target.to_string_lossy().into_owned();
            writes = Some(target);
        }
        let live = self.is_live(&job.handle);
        let from_csv = match &job.call {
            Call::Export(crate::ops::ExportArgs { format, .. }) => live && matches!(format.as_str(), "xlsx" | "xlsm" | "xlsb"),
            _ => false,
        };
        let handle = job.handle.clone();
        let mut out = if live { self.pump_live(relay, job).map(Some) } else { self.pump_model(relay, job) };
        if let (Ok(_), Some(t)) = (&out, writes) {
            if from_csv
                && let Some(next) = csv_saved_as_book(&handle, &t.to_string_lossy())
                && let Ok(Some(OpOut::Text { detail })) = &mut out
            {
                detail.push_str(&next);
            }
            self.written.insert(t);
        }
        out
    }

    /// Execute a job against the in-memory model.
    fn pump_model(&mut self, relay: &mut Relay, job: Job) -> Result<Option<OpOut>> {
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
    /// A document the application has closed: every handle naming it
    /// leaves the registry and its live binding, whatever unit it carries
    /// (`excel:plan.xlsx:workbook` and `excel:plan.xlsx:Sheet1` are one
    /// workbook). Left registered, the model would be shown a handle to
    /// nothing, and its next read would fail with "workbook not open".
    fn forget_document(&mut self, relay: &mut Relay, handle: &str) {
        let doc = |h: &str| {
            let mut p = h.splitn(3, ':');
            (p.next().unwrap_or("").to_string(), p.next().unwrap_or("").to_string())
        };
        let closed = doc(handle);
        for h in relay.registry(&self.session).unwrap_or_default() {
            if doc(&h) == closed {
                let _ = relay.detach(&self.session, &h);
                self.live.remove(&h);
            }
        }
    }

    fn pump_live(&mut self, relay: &mut Relay, job: Job) -> Result<OpOut> {
        let op = job.call.op();
        relay.emit(&self.session, "step.start", &job.handle, format!("{op:?} live"))?;
        // A verb this app does not have, refused here with the verbs it
        // does, rather than sent to the helper to come back "unsupported".
        if let Some(why) = crate::hand::envelope_for(&job.call, &job.handle)
            .and_then(|line| crate::json::parse(&line).ok())
            .and_then(|v| v.get("method").and_then(crate::json::Value::as_str).map(str::to_string))
            .and_then(|m| crate::tools::app_refuses(app_of(&job.handle), &m))
        {
            relay.emit(&self.session, "step.error", &job.handle, why.clone())?;
            return Err(Error::ClosedSchema(why));
        }
        if !job.call.gated() {
            relay.forget_calls(&self.session);
        } else if let Err(e) = relay.gate(&self.session, &format!("{op:?}"), &job.call.args_key(&job.handle)) {
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
                if matches!(&job.call, Call::Struct(crate::ops::StructArgs::Office { verb, .. }) if verb == "close") {
                    self.forget_document(relay, &job.handle);
                }
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

/// What finishes turning a CSV into a workbook, said after the export.
///
/// The export writes the workbook and leaves the CSV open, as an export
/// does. Nothing said so, and a model that then opened the workbook to work
/// in it left both open side by side: found in a live run, where the person
/// watching saw two files appear for one request. Closing the CSV is allowed
/// -- Syn opened it and it has no changes -- so the two calls that finish the
/// job are given ready to copy. None for anything but a `.csv` document.
fn csv_saved_as_book(handle: &str, written: &str) -> Option<String> {
    let mut parts = handle.splitn(3, ':');
    let (app, doc) = (parts.next()?, parts.next()?);
    if !doc.to_ascii_lowercase().ends_with(".csv") {
        return None;
    }
    let s = crate::json::s;
    let close = crate::json::obj(vec![("handle", s(handle)), ("verb", s("close"))]).to_json();
    let open = crate::json::obj(vec![("app", s(app)), ("path", s(written))]).to_json();
    Some(format!(
        "\n{doc} is still open beside it, unchanged. To go on in the workbook alone, close the CSV and open the workbook: struct{close} then open{open}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{FileContent, FileKind, OpenFile};

    #[test]
    fn a_csv_saved_as_a_workbook_says_how_to_close_it_and_open_the_book() {
        let next = csv_saved_as_book("excel:sales.csv:workbook", r"C:\data\sales.xlsx").unwrap();
        // Both calls are ones the tool parser takes as they stand.
        let call = |name: &str, tail: &str| {
            let json = &tail[..tail.find('}').unwrap() + 1];
            crate::tools::to_action(&crate::tools::ToolCall { id: "1".into(), name: name.into(), arguments: json.into() })
        };
        let after_struct = next.split("struct").nth(1).unwrap();
        let after_open = next.split(" then open").nth(1).unwrap();
        assert!(call("struct", after_struct).is_ok(), "{next}");
        assert!(call("open", after_open).is_ok(), "{next}");
        assert!(after_struct.starts_with(r#"{"handle":"excel:sales.csv:workbook","verb":"close"}"#), "{next}");
        assert!(after_open.starts_with(r#"{"app":"excel","path":"C:\\data\\sales.xlsx"}"#), "{next}");
        // Anything that is not a CSV gets nothing: there is nothing to finish.
        assert_eq!(csv_saved_as_book("excel:sales.xlsx:workbook", r"C:\data\copy.xlsx"), None);
        assert!(csv_saved_as_book("excel:SALES.CSV:workbook", r"C:\d\s.xlsx").is_some(), "the extension in any case");
    }
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

    fn export_job(h: &str, path: &str) -> Job {
        Job {
            handle: h.into(),
            summary: "export".into(),
            call: Call::Export(crate::ops::ExportArgs { format: "xlsx".into(), path: Some(path.into()), sheet: None }),
        }
    }

    #[test]
    fn an_export_stays_in_the_workspace_and_never_replaces_a_file_it_did_not_write() {
        // A document that says "export this over C:\...\thesis.docx" had a
        // model overwrite the person's own file: export wrote anywhere, and
        // SaveCopyAs replaces what is there.
        let base = std::env::temp_dir().join(format!("syn-export-{}", std::process::id()));
        let ws = base.join("ws");
        let away = base.join("elsewhere");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&away).unwrap();
        std::fs::write(ws.join("theirs.xlsx"), b"the person's own").unwrap();
        let (mut relay, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.confine(vec![ws.clone()]);
        let mut go = |run: &mut Runner, path: &str| {
            run.submit(export_job(&h, path));
            run.pump(&mut relay)
        };

        let outside = away.join("copy.xlsx").to_string_lossy().into_owned();
        assert!(matches!(go(&mut run, &outside), Err(Error::Denied(w)) if w.contains("outside")), "outside the workspace");
        let sneaky = ws.join("..").join("elsewhere").join("copy.xlsx").to_string_lossy().into_owned();
        assert!(matches!(go(&mut run, &sneaky), Err(Error::Denied(w)) if w.contains("outside")), ".. does not lead out");
        let theirs = ws.join("theirs.xlsx").to_string_lossy().into_owned();
        assert!(matches!(go(&mut run, &theirs), Err(Error::Denied(w)) if w.contains("already exists")), "not over their file");
        assert_eq!(std::fs::read(ws.join("theirs.xlsx")).unwrap(), b"the person's own");

        // A relative path is under the workspace, and a file this session
        // wrote may be written again.
        assert!(go(&mut run, "copy.xlsx").is_ok());
        assert!(ws.join("copy.xlsx").is_file());
        assert!(go(&mut run, "copy.xlsx").is_ok(), "its own export may be replaced");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn with_no_workspace_a_relative_export_is_refused_rather_than_put_somewhere() {
        let (mut relay, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.submit(export_job(&h, "copy.xlsx"));
        assert!(matches!(run.pump(&mut relay), Err(Error::Denied(w)) if w.contains("not a full path")));
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
    fn a_verb_the_app_does_not_have_is_refused_before_the_helper_with_the_ones_it_does() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.attach_hand(Box::new(FakeHand::ok(&["the helper was asked"])));
        run.mark_live(&h).unwrap();
        run.submit(Job {
            handle: h.clone(),
            summary: "move".into(),
            call: Call::Struct(crate::ops::StructArgs::Office {
                verb: "moveSlide".into(),
                args: vec![("selector".into(), "s2".into()), ("at".into(), "s1".into())],
                payload: String::new(),
            }),
        });
        match run.pump(&mut r) {
            Err(Error::ClosedSchema(why)) => {
                assert!(why.contains("PowerPoint only"), "{why}");
                assert!(why.contains("Excel's struct verbs are:") && why.contains("sort"), "{why}");
            }
            other => panic!("moveSlide on a workbook must be refused before the helper, got {other:?}"),
        }
        // A verb Excel has still goes through.
        run.submit(read_job(&h));
        assert_eq!(run.pump(&mut r).unwrap().unwrap(), OpOut::Text { detail: "the helper was asked".into() });
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
    fn a_chart_comes_only_from_a_workbook_this_session_was_given_and_the_policy_allows() {
        let (mut r, s, xl) = relay1();
        let doc = crate::protocol::new_handle("word", "m.docx", "body");
        r.attach(&s, doc.clone(), OpenFile {
            kind: FileKind::Word,
            content: FileContent::Word { paras: vec![], tables: vec![], changes: vec![], comments: vec![] },
            styles: HashMap::new(),
        });
        let embed = |from: &str| Job {
            handle: doc.clone(),
            summary: "chart".into(),
            call: Call::Struct(crate::ops::StructArgs::Office {
                verb: "embedChart".into(),
                args: vec![("from".into(), from.into()), ("source".into(), "Sheet1!1".into())],
                payload: String::new(),
            }),
        };
        let mut run = Runner::new(&s);
        run.attach_hand(Box::new(FakeHand::ok(&["chart Sheet1!1 added"])));
        run.mark_live(&doc).unwrap();
        // Named by a workbook nobody gave the model: never found by name.
        run.submit(embed("excel:payroll.xlsx:workbook"));
        assert!(matches!(run.pump(&mut r), Err(Error::UnknownHandle(h)) if h == "excel:payroll.xlsx:workbook"));
        // A registered workbook, with Excel off by policy: still refused.
        run.lock_allowlist(vec!["word".into()]);
        run.submit(embed(&xl));
        assert!(matches!(run.pump(&mut r), Err(Error::AppDenied(_))));
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
    fn the_same_read_of_three_documents_is_not_a_loop_and_undo_never_is() {
        // Found by the live MCP test: three summaries of three different
        // documents were refused as one call made three times, and three
        // undos in a row -- three changes taken back -- would have been too.
        let (mut r, s, h) = relay1();
        for name in ["q.xlsx", "z.xlsx"] {
            r.attach(&s, crate::protocol::new_handle("excel", name, "Sheet1"), OpenFile {
                kind: FileKind::Excel,
                content: FileContent::Excel { sheets: HashMap::from([("Sheet1".into(), vec![vec!["1".into()]])]) },
                styles: HashMap::new(),
            });
        }
        let mut run = Runner::new(&s);
        for name in ["p.xlsx", "q.xlsx", "z.xlsx"] {
            let job = Job { handle: format!("excel:{name}:Sheet1"), summary: "read".into(), call: Call::Read(ReadArgs { selector: "Sheet1".into() }) };
            assert!(run.run(&mut r, &job.handle.clone(), "read", job.call).is_ok(), "{name}");
        }
        for _ in 0..3 {
            let out = run.run(&mut r, &h, "undo", Call::Undo);
            assert!(!matches!(out, Err(Error::DoomLoop(_))), "undo three times running is not a loop");
        }
        // And the same read either side of an undo is two looks at two
        // different documents.
        let read = || Call::Read(ReadArgs { selector: "Sheet1".into() });
        assert!(run.run(&mut r, &h, "read", read()).is_ok());
        assert!(run.run(&mut r, &h, "read", read()).is_ok());
        let _ = run.run(&mut r, &h, "undo", Call::Undo);
        assert!(run.run(&mut r, &h, "read", read()).is_ok(), "the undo in between makes this a new look");
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
    fn a_call_made_while_paused_never_runs_later() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        let read = |sel: &str| Call::Read(ReadArgs { selector: sel.into() });
        for _ in 0..3 {
            let _ = run.run(&mut r, &h, "read", read("Sheet1"));
        }
        assert_eq!(run.state(), &QueueState::Paused, "three identical calls freeze the run");
        assert!(run.run(&mut r, &h, "read", read("Sheet1!A1")).unwrap().is_none());
        assert_eq!(run.pending(), 0, "a refused call must not wait behind the pause");
        run.thaw(&mut r);
        let out = run.run(&mut r, &h, "read", read("Sheet1!B1")).unwrap().unwrap();
        assert!(format!("{out:?}").contains("1x1"), "the call after a thaw gets its own answer: {out:?}");
    }

    #[test]
    fn thawing_never_undoes_the_kill_switch() {
        let (mut r, s, h) = relay1();
        let mut run = Runner::new(&s);
        run.kill();
        run.thaw(&mut r);
        assert!(matches!(run.run(&mut r, &h, "read", Call::Read(ReadArgs { selector: "Sheet1".into() })), Err(Error::Killed)));
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
