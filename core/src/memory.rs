//! Memory: session learning + cross-app recall (M2/M3).
//! Per-session facts the agent accumulates ("user prefers Calibri 11",
//! "Q3 lives in Sheet2") plus a global transfer log so context that moved
//! from Excel to Word can be recalled when Word asks for it later.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Transfer {
    pub seq: u64,
    pub from_handle: String,
    pub to_handle: String,
    pub summary: String,
}

#[derive(Debug, Default)]
pub struct Memory {
    facts: HashMap<(String, String), String>,
    transfers: Vec<Transfer>,
    seq: u64,
}

impl Memory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Learn one fact about a session. Overwrites the same key.
    pub fn remember(&mut self, session: &str, key: &str, value: &str) {
        self.facts.insert((session.to_string(), key.to_string()), value.to_string());
    }

    pub fn recall(&self, session: &str, key: &str) -> Option<&str> {
        self.facts.get(&(session.to_string(), key.to_string())).map(String::as_str)
    }

    pub fn forget(&mut self, session: &str, key: &str) -> bool {
        self.facts.remove(&(session.to_string(), key.to_string())).is_some()
    }

    /// Log one cross-app movement (xfer op). Returns the transfer id.
    pub fn log_transfer(&mut self, from_handle: &str, to_handle: &str, summary: &str) -> u64 {
        self.seq += 1;
        self.transfers.push(Transfer {
            seq: self.seq,
            from_handle: from_handle.into(),
            to_handle: to_handle.into(),
            summary: summary.into(),
        });
        self.seq
    }

    /// Most recent transfers first, bounded.
    pub fn recent(&self, n: usize) -> Vec<&Transfer> {
        self.transfers.iter().rev().take(n).collect()
    }

    /// Session-learning compaction: facts + recent transfer ids in one
    /// short string suitable for a context packet.
    pub fn compact(&self, session: &str) -> String {
        let mut facts: Vec<_> = self
            .facts
            .iter()
            .filter(|((s, _), _)| s == session)
            .map(|((_, k), v)| format!("{k}={v}"))
            .collect();
        facts.sort();
        format!("session {session}: {} facts [{}]; {} transfers total", facts.len(), facts.join(", "), self.transfers.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_recall_forget() {
        let mut m = Memory::new();
        m.remember("s", "font", "Calibri 11");
        assert_eq!(m.recall("s", "font"), Some("Calibri 11"));
        assert_eq!(m.recall("other", "font"), None);
        assert!(m.forget("s", "font"));
        assert!(!m.forget("s", "font"));
    }

    #[test]
    fn transfer_log_and_compact() {
        let mut m = Memory::new();
        m.remember("s", "q3", "Sheet2");
        m.log_transfer("excel:plan.xlsx:S1", "word:report.docx:Body", "copied Q3 table");
        m.log_transfer("word:report.docx:Body", "ppt:deck.pptx:S2", "past summary");
        let recent = m.recent(1);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].seq, 2);
        let c = m.compact("s");
        assert!(c.contains("1 facts") && c.contains("q3=Sheet2") && c.contains("2 transfers"));
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    #[test]
    fn overwrite_and_empty_compact() {
        let mut m = Memory::new();
        m.remember("s", "k", "v1");
        m.remember("s", "k", "v2");
        assert_eq!(m.recall("s", "k"), Some("v2"));
        assert_eq!(m.recent(5).len(), 0);
        let c = m.compact("empty");
        assert!(c.contains("0 facts") && c.contains("0 transfers"));
    }

    #[test]
    fn transfer_ids_sequence_and_bounds() {
        let mut m = Memory::new();
        assert_eq!(m.log_transfer("a", "b", "first"), 1);
        assert_eq!(m.log_transfer("b", "c", "second"), 2);
        assert_eq!(m.recent(99).len(), 2);
        assert_eq!(m.recent(0).len(), 0);
        let r = m.recent(2);
        assert_eq!(r[0].summary, "second"); // newest first
        assert_eq!(r[1].from_handle, "a");
        assert_eq!(r[1].to_handle, "b");
    }
}
