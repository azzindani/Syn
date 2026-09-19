//! Sessions: concurrent multi-session orchestration (M5).
//! One SessionHub owns a Runner per session id. Turns interleave — the
//! widget pumps session A while session B stays paused, and cancelling one
//! run never touches the others.

use crate::bus::Relay;
use crate::ops::OpOut;
use crate::protocol::Result;
use crate::queue::QueueState;
use crate::runner::{Job, Runner};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct SessionHub {
    runners: HashMap<String, Runner>,
}

impl SessionHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the runner for a session, creating it on first use.
    pub fn open(&mut self, session: &str) -> &mut Runner {
        self.runners.entry(session.to_string()).or_insert_with(|| Runner::new(session))
    }

    pub fn submit(&mut self, session: &str, job: Job) -> String {
        self.open(session).submit(job)
    }

    pub fn pump(&mut self, relay: &mut Relay, session: &str) -> Result<Option<OpOut>> {
        self.open(session).pump(relay)
    }

    pub fn pause(&mut self, session: &str) {
        self.open(session).pause();
    }

    pub fn cancel(&mut self, session: &str) {
        if let Some(r) = self.runners.get_mut(session) {
            r.cancel();
        }
    }

    /// Resume after revalidating handles. Returns vanished handles.
    pub fn resume(&mut self, relay: &Relay, session: &str) -> Result<Vec<String>> {
        self.open(session).resume(relay)
    }

    /// Drop a finished session entirely. Returns false if unknown.
    pub fn drop(&mut self, session: &str) -> bool {
        self.runners.remove(session).is_some()
    }

    /// (session id, queue state, pending jobs) for the widget session list.
    pub fn list(&self) -> Vec<(String, QueueState, usize)> {
        let mut out: Vec<_> = self
            .runners
            .iter()
            .map(|(id, r)| (id.clone(), r.state().clone(), r.pending()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{FileContent, FileKind, OpenFile, Relay};
    use crate::ops::{Call, ReadArgs};
    use std::collections::HashMap;

    fn relay2() -> Relay {
        let mut r = Relay::new();
        r.handshake("a", "t");
        r.handshake("b", "t");
        r.attach("a", "excel:a.xlsx:Sheet1".into(), OpenFile {
            kind: FileKind::Excel,
            content: FileContent::Excel { sheets: HashMap::from([("Sheet1".into(), vec![vec!["1".into()]])]) },
            styles: HashMap::new(),
        });
        r.attach("b", "word:b.docx:Body".into(), OpenFile {
            kind: FileKind::Word,
            content: FileContent::Word { paras: vec!["hi".into()], tables: vec![], changes: vec![], comments: vec![] },
            styles: HashMap::new(),
        });
        r
    }

    fn read_job(handle: &str) -> Job {
        let selector = if handle.starts_with("word:") { "body" } else { "Sheet1" };
        Job {
            handle: handle.into(),
            summary: "read".into(),
            call: Call::Read(ReadArgs { selector: selector.into() }),
        }
    }

    #[test]
    fn sessions_are_independent() {
        let mut hub = SessionHub::new();
        let mut r = relay2();
        hub.submit("a", read_job("excel:a.xlsx:Sheet1"));
        hub.submit("b", read_job("word:b.docx:Body"));
        hub.cancel("a");
        // B still pumps fine after A was cancelled.
        assert!(hub.pump(&mut r, "b").unwrap().is_some());
        assert!(hub.pump(&mut r, "a").unwrap().is_none());
    }

    #[test]
    fn list_and_drop() {
        let mut hub = SessionHub::new();
        hub.submit("s2", read_job("h"));
        hub.submit("s1", read_job("h"));
        let ids: Vec<_> = hub.list().into_iter().map(|(id, _, _)| id).collect();
        assert_eq!(ids, vec!["s1".to_string(), "s2".to_string()]);
        assert!(hub.drop("s1"));
        assert!(!hub.drop("s1"));
        assert_eq!(hub.list().len(), 1);
    }

    #[test]
    fn unknown_session_pump_is_empty() {
        let mut hub = SessionHub::new();
        let mut r = Relay::new();
        assert!(hub.pump(&mut r, "ghost").unwrap().is_none());
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;
    use crate::bus::{FileContent, FileKind, OpenFile, Relay};
    use crate::ops::{Call, ReadArgs};
    use std::collections::HashMap;

    fn relay_w() -> Relay {
        let mut r = Relay::new();
        r.handshake("w", "t");
        r.attach("w", "word:d.docx:body".into(), OpenFile {
            kind: FileKind::Word,
            content: FileContent::Word { paras: vec!["hi".into()], tables: vec![], changes: vec![], comments: vec![] },
            styles: HashMap::new(),
        });
        r
    }

    #[test]
    fn pause_resume_roundtrip() {
        let mut hub = SessionHub::new();
        let mut r = relay_w();
        hub.submit("w", Job {
            handle: "word:d.docx:body".into(), summary: "read".into(),
            call: Call::Read(ReadArgs { selector: "body".into() }),
        });
        hub.pause("w");
        assert!(hub.pump(&mut r, "w").unwrap().is_none());
        let missing = hub.resume(&r, "w").unwrap();
        assert!(missing.is_empty());
        assert!(hub.pump(&mut r, "w").unwrap().is_some());
    }

    #[test]
    fn resume_unknown_session_errors() {
        let mut hub = SessionHub::new();
        let r = Relay::new(); // nothing attached
        hub.submit("g", Job {
            handle: "excel:gone.xlsx:S".into(), summary: "read".into(),
            call: Call::Read(ReadArgs { selector: "S".into() }),
        });
        hub.pause("g");
        assert!(hub.resume(&r, "g").is_err()); // unknown session: handshake first
    }

    #[test]
    fn cancel_unknown_is_quiet() {
        let mut hub = SessionHub::new();
        hub.cancel("nobody"); // must not panic
        assert!(hub.list().is_empty());
    }
}
