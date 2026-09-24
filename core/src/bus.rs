//! Relay bus: sessions, file-handle registry, event stream, snapshots,
//! doom-loop gate. Single-user, single-device: no auth, no tenants.
use std::collections::HashMap;

use crate::protocol::{DOOM_LOOP_THRESHOLD, Error, Result, new_handle};

pub use crate::protocol::new_handle as make_handle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Excel,
    Word,
    Ppt,
}

/// In-memory file models. COM / Office.js backends implement the same ops
/// against live documents later; the protocol never changes.
#[derive(Debug, Clone, PartialEq)]
pub enum FileContent {
    Excel { sheets: HashMap<String, Vec<Vec<String>>> },
    Word { paras: Vec<String>, tables: Vec<Vec<Vec<String>>>, changes: Vec<String>, comments: Vec<String> },
    Ppt { slides: Vec<Slide> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Slide {
    pub title: String,
    pub bullets: Vec<String>,
    pub provenance: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpenFile {
    pub kind: FileKind,
    pub content: FileContent,
    pub styles: HashMap<String, Vec<(String, String)>>,
}

impl OpenFile {
    /// An empty workbook with one 4x4 `Sheet1`: what `attach` registers
    /// when no live hand is involved, so ops have a model to act on.
    pub fn blank_excel() -> Self {
        OpenFile {
            kind: FileKind::Excel,
            content: FileContent::Excel { sheets: HashMap::from([("Sheet1".into(), vec![vec![String::new(); 4]; 4])]) },
            styles: HashMap::new(),
        }
    }

    pub fn blank_word() -> Self {
        OpenFile {
            kind: FileKind::Word,
            content: FileContent::Word { paras: vec![], tables: vec![], changes: vec![], comments: vec![] },
            styles: HashMap::new(),
        }
    }

    pub fn blank_ppt() -> Self {
        OpenFile { kind: FileKind::Ppt, content: FileContent::Ppt { slides: vec![] }, styles: HashMap::new() }
    }

    /// The registry entry for a document a live hand holds. The model in
    /// memory is never read for a live handle -- the hand is -- so it stays
    /// empty, rather than filled with something an op could mistake for the
    /// real document.
    pub fn placeholder(app: &str) -> Self {
        match app {
            "excel" => Self::blank_excel(),
            "ppt" => Self::blank_ppt(),
            _ => Self::blank_word(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Event {
    pub t: String,
    pub handle: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DocSnapshot {
    pub document_id: String,
    pub instance_id: String,
    pub app: String,
    pub tools: Vec<String>,
    pub host: String,
}

#[derive(Debug, Default)]
struct Session {
    files: HashMap<String, OpenFile>,
    events: Vec<Event>,
    calls: Vec<(String, String)>,
    snapshots: HashMap<String, Vec<FileContent>>,
    docs: HashMap<String, DocSnapshot>,
    follow: HashMap<String, String>,
}

#[derive(Debug, Default)]
pub struct Relay {
    sessions: HashMap<String, Session>,
}

impl Relay {
    pub fn new() -> Self {
        Self::default()
    }

    fn session(&self, id: &str) -> Result<&Session> {
        self.sessions.get(id).ok_or_else(|| Error::UnknownSession(id.to_string()))
    }

    fn session_mut(&mut self, id: &str) -> Result<&mut Session> {
        self.sessions.get_mut(id).ok_or_else(|| Error::UnknownSession(id.to_string()))
    }

    /// Admit (or rejoin) a session. Idempotent like opencode session adopt.
    pub fn handshake(&mut self, id: &str, client: &str) -> Vec<String> {
        let s = self.sessions.entry(id.to_string()).or_default();
        s.events.push(Event { t: "session.open".into(), handle: String::new(), detail: client.into() });
        let mut files: Vec<String> = s.files.keys().cloned().collect();
        files.sort();
        files
    }

    pub fn ping(&self) -> usize {
        self.sessions.len()
    }

    pub fn attach(&mut self, session: &str, handle: String, file: OpenFile) {
        let s = self.sessions.entry(session.to_string()).or_default();
        s.events.push(Event { t: "file.attach".into(), handle: handle.clone(), detail: format!("{:?}", file.kind) });
        s.files.insert(handle.clone(), file);
        s.snapshots.entry(handle).or_default();
    }

    /// Hello/snapshot handshake (office-agents bridge pattern): the pane
    /// announces document identity + offered tools + host; relay records it.
    pub fn attach_with_snapshot(&mut self, session: &str, handle: String, file: OpenFile, snap: DocSnapshot) {
        let detail = format!("doc={} app={} tools={}", snap.document_id, snap.app, snap.tools.len());
        self.attach(session, handle.clone(), file);
        if let Ok(s) = self.session_mut(session) {
            s.docs.insert(handle.clone(), snap);
            s.events.push(Event { t: "file.hello".into(), handle, detail });
        }
    }

    /// Follow-mode: record the pane's current selection into the live feed.
    pub fn follow(&mut self, session: &str, handle: &str, selection: &str) -> Result<()> {
        let s = self.session_mut(session)?;
        if !s.files.contains_key(handle) {
            return Err(Error::UnknownHandle(handle.into()));
        }
        s.follow.insert(handle.to_string(), selection.to_string());
        s.events.push(Event { t: "follow".into(), handle: handle.into(), detail: selection.into() });
        Ok(())
    }

    pub fn selection(&self, session: &str, handle: &str) -> Result<Option<String>> {
        Ok(self.session(session)?.follow.get(handle).cloned())
    }

    /// Forget one document. Its snapshots go with it: an undo stack for a
    /// handle nobody can address is memory held for nothing.
    pub fn detach(&mut self, session: &str, handle: &str) -> Result<()> {
        let s = self.session_mut(session)?;
        s.files.remove(handle);
        s.snapshots.remove(handle);
        Ok(())
    }

    pub fn registry(&self, session: &str) -> Result<Vec<String>> {
        let mut files: Vec<String> = self.session(session)?.files.keys().cloned().collect();
        files.sort();
        Ok(files)
    }

    pub fn emit(&mut self, session: &str, t: &str, handle: &str, detail: String) -> Result<()> {
        self.session_mut(session)?.events.push(Event { t: t.into(), handle: handle.into(), detail });
        Ok(())
    }

    pub fn events(&self, session: &str) -> Result<Vec<Event>> {
        Ok(self.session(session)?.events.clone())
    }

    /// Snapshot before every mutation (opencode pre-stream snapshot rule).
    pub fn snapshot(&mut self, session: &str, handle: &str) -> Result<usize> {
        let s = self.session_mut(session)?;
        let content = s.files.get(handle).ok_or_else(|| Error::UnknownHandle(handle.into()))?.content.clone();
        let stack = s.snapshots.entry(handle.to_string()).or_default();
        stack.push(content);
        Ok(stack.len())
    }

    pub fn undo(&mut self, session: &str, handle: &str) -> Result<usize> {
        let s = self.session_mut(session)?;
        let stack = s.snapshots.entry(handle.to_string()).or_default();
        let content = stack.pop().ok_or_else(|| Error::EmptyUndo(handle.into()))?;
        let file = s.files.get_mut(handle).ok_or_else(|| Error::UnknownHandle(handle.into()))?;
        file.content = content;
        let remaining = stack.len();
        s.events.push(Event { t: "step.undo".into(), handle: handle.into(), detail: format!("remaining={remaining}") });
        Ok(remaining)
    }

    /// Doom-loop gate: 3 identical consecutive (op, args) calls need confirm.
    pub fn gate(&mut self, session: &str, op: &str, args_key: &str) -> Result<()> {
        let s = self.session_mut(session)?;
        s.calls.push((op.to_string(), args_key.to_string()));
        let n = s.calls.len();
        if n >= DOOM_LOOP_THRESHOLD {
            let last = &s.calls[n - DOOM_LOOP_THRESHOLD..];
            if last.iter().all(|c| c == &last[0]) {
                return Err(Error::DoomLoop(op.to_string()));
            }
        }
        Ok(())
    }

    pub(crate) fn files_mut(&mut self, session: &str) -> Result<&mut HashMap<String, OpenFile>> {
        Ok(&mut self.session_mut(session)?.files)
    }

    pub(crate) fn snapshot_len(&self, session: &str, handle: &str) -> usize {
        self.sessions.get(session).and_then(|s| s.snapshots.get(handle)).map(|v| v.len()).unwrap_or(0)
    }
}

/// Helper re-export so call sites read naturally.
pub fn handle(app: &str, file: &str, unit: &str) -> String {
    new_handle(app, file, unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn excel() -> OpenFile {
        OpenFile {
            kind: FileKind::Excel,
            content: FileContent::Excel {
                sheets: HashMap::from([("Sheet1".into(), vec![vec!["1".into(), "2".into()], vec!["3".into(), "4".into()]])]),
            },
            styles: HashMap::new(),
        }
    }

    #[test]
    fn handshake_rejoin_lists_files() {
        let mut r = Relay::new();
        assert!(r.handshake("s", "widget").is_empty());
        r.attach("s", handle("excel", "p.xlsx", "Sheet1"), excel());
        assert_eq!(r.handshake("s", "widget"), vec!["excel:p.xlsx:Sheet1".to_string()]);
    }

    #[test]
    fn unknown_session_rejected() {
        let r = Relay::new();
        assert!(matches!(r.registry("nope"), Err(Error::UnknownSession(_))));
    }

    #[test]
    fn undo_restores_snapshot() {
        let mut r = Relay::new();
        r.handshake("s", "w");
        let h = handle("excel", "p.xlsx", "Sheet1");
        r.attach("s", h.clone(), excel());
        r.snapshot("s", &h).unwrap();
        if let FileContent::Excel { sheets } = &mut r.sessions.get_mut("s").unwrap().files.get_mut(&h).unwrap().content {
            sheets.get_mut("Sheet1").unwrap()[0][0] = "9".to_string();
        }
        r.undo("s", &h).unwrap();
        if let FileContent::Excel { sheets } = &r.sessions["s"].files[&h].content {
            assert_eq!(sheets["Sheet1"][0][0], "1");
        }
    }

    #[test]
    fn doom_loop_trips_on_third_identical() {
        let mut r = Relay::new();
        r.handshake("s", "w");
        assert!(r.gate("s", "read", "k").is_ok());
        assert!(r.gate("s", "read", "k").is_ok());
        assert!(matches!(r.gate("s", "read", "k"), Err(Error::DoomLoop(_))));
    }

    #[test]
    fn hello_snapshot_and_follow() {
        let mut r = Relay::new();
        r.handshake("s", "w");
        let h = handle("word", "d.docx", "body");
        r.attach_with_snapshot("s", h.clone(), excel(), DocSnapshot {
            document_id: "doc-1".into(),
            instance_id: "inst-1".into(),
            app: "word".into(),
            tools: vec!["read".into(), "write".into()],
            host: "winword/16.0".into(),
        });
        r.follow("s", &h, "p3").unwrap();
        assert_eq!(r.selection("s", &h).unwrap(), Some("p3".to_string()));
        let kinds: Vec<String> = r.events("s").unwrap().iter().map(|e| e.t.clone()).collect();
        assert!(kinds.contains(&"file.hello".to_string()));
        assert!(kinds.contains(&"follow".to_string()));
        assert!(r.follow("s", "word:nope:body", "p1").is_err());
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;
    use std::collections::HashMap;

    fn excel_file() -> OpenFile {
        OpenFile {
            kind: FileKind::Excel,
            content: FileContent::Excel {
                sheets: HashMap::from([("Sheet1".into(), vec![vec!["1".into()]])]) },
            styles: HashMap::new(),
        }
    }

    #[test]
    fn ping_counts_and_attach_autocreates() {
        let mut r = Relay::new();
        assert_eq!(r.ping(), 0);
        r.attach("fresh", "excel:a.xlsx:S1".into(), excel_file()); // no handshake needed
        assert_eq!(r.ping(), 1);
        assert_eq!(r.registry("fresh").unwrap(), vec!["excel:a.xlsx:S1".to_string()]);
    }

    #[test]
    fn events_keep_order_with_session_open_first() {
        let mut r = Relay::new();
        r.handshake("s", "widget");
        r.attach("s", "excel:a.xlsx:S1".into(), excel_file());
        r.emit("s", "custom", "h", "d".into()).unwrap();
        let kinds: Vec<_> = r.events("s").unwrap().into_iter().map(|e| e.t).collect();
        assert_eq!(kinds, vec!["session.open", "file.attach", "custom"]);
        assert!(r.emit("ghost", "x", "h", "d".into()).is_err());
    }

    #[test]
    fn registry_sorts_and_snapshot_len_grows() {
        let mut r = Relay::new();
        r.handshake("s", "w");
        r.attach("s", "word:b.docx:body".into(), excel_file());
        r.attach("s", "excel:a.xlsx:S1".into(), excel_file());
        assert_eq!(r.registry("s").unwrap()[0], "excel:a.xlsx:S1");
        assert_eq!(r.snapshot_len("s", "excel:a.xlsx:S1"), 0);
        r.snapshot("s", "excel:a.xlsx:S1").unwrap();
        r.snapshot("s", "excel:a.xlsx:S1").unwrap();
        assert_eq!(r.snapshot_len("s", "excel:a.xlsx:S1"), 2);
    }

    #[test]
    fn empty_undo_errors_and_gate_allows_variety() {
        let mut r = Relay::new();
        r.handshake("s", "w");
        r.attach("s", "h".into(), excel_file());
        assert!(matches!(r.undo("s", "h"), Err(Error::EmptyUndo(_))));
        // Same op, differing args: never a loop. Two same + different resets.
        for k in ["a", "b", "a", "a", "c", "a", "a"] {
            assert!(r.gate("s", "read", k).is_ok(), "{k}");
        }
        assert_eq!(r.selection("s", "h").unwrap(), None);
    }
}
