//! Disk-persisted snapshot index (ai-office-mcp SnapshotUndo pattern).
//! Snapshot bytes live in `<dir>/<id>.snap`; `index.dat` maps ids to handles.
//! Orphan rows (bytes file gone) are pruned on load. Std only, no serde:
//! index lines are `id\thandle\tts` (handles must not contain tabs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub id: String,
    pub handle: String,
    pub ts: u64,
}

#[derive(Debug)]
pub struct SnapshotIndex {
    dir: PathBuf,
    entries: Vec<SnapshotEntry>,
    counter: u64,
}

impl SnapshotIndex {
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let mut entries = Vec::new();
        let index = dir.join("index.dat");
        if index.exists() {
            let text = std::fs::read_to_string(&index)?;
            for line in text.lines() {
                let mut parts = line.split('\t');
                if let (Some(id), Some(handle), Some(ts)) = (parts.next(), parts.next(), parts.next())
                    && dir.join(format!("{id}.snap")).exists()
                {
                    entries.push(SnapshotEntry { id: id.into(), handle: handle.into(), ts: ts.parse().unwrap_or(0) });
                }
            }
        }
        let counter = entries.len() as u64;
        Ok(Self { dir: dir.into(), entries, counter })
    }

    fn save(&self) -> std::io::Result<()> {
        let mut text = String::new();
        for e in &self.entries {
            text.push_str(&format!("{}\t{}\t{}\n", e.id, e.handle.replace('\t', " "), e.ts));
        }
        std::fs::write(self.dir.join("index.dat"), text)
    }

    fn now_ts() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    }

    pub fn push(&mut self, handle: &str, bytes: &[u8]) -> std::io::Result<String> {
        self.counter += 1;
        let id = format!("snap{:06}", self.counter);
        std::fs::write(self.dir.join(format!("{id}.snap")), bytes)?;
        self.entries.push(SnapshotEntry { id: id.clone(), handle: handle.into(), ts: Self::now_ts() });
        self.save()?;
        Ok(id)
    }

    pub fn list(&self, handle: &str) -> Vec<&SnapshotEntry> {
        self.entries.iter().filter(|e| e.handle == handle).collect()
    }

    pub fn get(&self, id: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(self.dir.join(format!("{id}.snap")))
    }

    /// Drop index rows whose bytes are gone; returns pruned count.
    pub fn prune_orphans(&mut self) -> std::io::Result<usize> {
        let before = self.entries.len();
        let dir = self.dir.clone();
        self.entries.retain(|e| dir.join(format!("{}.snap", e.id)).exists());
        self.save()?;
        Ok(before - self.entries.len())
    }

    pub fn handles(&self) -> HashMap<String, usize> {
        let mut m = HashMap::new();
        for e in &self.entries {
            *m.entry(e.handle.clone()).or_insert(0) += 1;
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("harness-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn push_list_get_roundtrip() {
        let d = tmpdir("snap1");
        let mut idx = SnapshotIndex::open(&d).unwrap();
        let id = idx.push("excel:a.xlsx:S1", b"bytes1").unwrap();
        assert_eq!(idx.list("excel:a.xlsx:S1").len(), 1);
        assert_eq!(idx.get(&id).unwrap(), b"bytes1");
        // Reopen: index survives restarts.
        let idx2 = SnapshotIndex::open(&d).unwrap();
        assert_eq!(idx2.list("excel:a.xlsx:S1").len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn prune_orphans() {
        let d = tmpdir("snap2");
        let mut idx = SnapshotIndex::open(&d).unwrap();
        let id = idx.push("h", b"x").unwrap();
        std::fs::remove_file(d.join(format!("{id}.snap"))).unwrap();
        assert_eq!(idx.prune_orphans().unwrap(), 1);
        assert!(idx.list("h").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    fn tmpdir2(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("harness-cover-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn handles_counts_and_ids_keep_growing() {
        let d = tmpdir2("handles");
        let mut idx = SnapshotIndex::open(&d).unwrap();
        let id1 = idx.push("a", b"1").unwrap();
        idx.push("a", b"2").unwrap();
        idx.push("b", b"3").unwrap();
        let h = idx.handles();
        assert_eq!(h.get("a"), Some(&2));
        assert_eq!(h.get("b"), Some(&1));
        let idx2 = SnapshotIndex::open(&d).unwrap();
        let mut idx2 = idx2;
        let id2 = idx2.push("a", b"4").unwrap();
        assert_ne!(id1, id2); // counter resumes past persisted rows
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn malformed_index_lines_ignored() {
        let d = tmpdir2("malformed");
        let mut idx = SnapshotIndex::open(&d).unwrap();
        idx.push("h", b"x").unwrap();
        std::fs::write(d.join("index.dat"), "garbage-no-tabs\n\n\th\t\n").unwrap();
        let idx2 = SnapshotIndex::open(&d).unwrap();
        assert!(idx2.list("h").is_empty()); // orphaned bytes row pruned by reload
        assert_eq!(idx2.handles().len(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn get_missing_is_io_error() {
        let d = tmpdir2("missing");
        let idx = SnapshotIndex::open(&d).unwrap();
        assert!(idx.get("snap999999").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
