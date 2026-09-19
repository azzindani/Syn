//! Confined VFS (office-agents bridge vfs_* pattern): list/read/write/delete
//! scoped under one root. Absolute escapes and `..`-past-root are rejected.
//! Std only.

use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub enum VfsError {
    Escape(String),
    Io(std::io::Error),
}

impl std::fmt::Display for VfsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Escape(p) => write!(f, "path escapes workspace root: {p}"),
            Self::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl From<std::io::Error> for VfsError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, VfsError>;

#[derive(Debug)]
pub struct Vfs {
    root: PathBuf,
}

impl Vfs {
    pub fn open(root: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(root)?;
        Ok(Self { root: root.into() })
    }

    fn resolve(&self, path: &str) -> Result<PathBuf> {
        let mut out = self.root.clone();
        for c in Path::new(path).components() {
            match c {
                Component::Normal(s) => out.push(s),
                Component::CurDir => {}
                Component::ParentDir => {
                    if !out.pop() {
                        return Err(VfsError::Escape(path.into()));
                    }
                    if out != self.root && !out.starts_with(&self.root) {
                        return Err(VfsError::Escape(path.into()));
                    }
                }
                _ => return Err(VfsError::Escape(path.into())),
            }
        }
        if out != self.root && !out.starts_with(&self.root) {
            return Err(VfsError::Escape(path.into()));
        }
        Ok(out)
    }

    pub fn list(&self, prefix: &str) -> Result<Vec<(String, u64)>> {
        let dir = self.resolve(prefix)?;
        let mut out = Vec::new();
        if dir.is_dir() {
            for e in std::fs::read_dir(&dir)? {
                let e = e?;
                let rel = e.path().strip_prefix(&self.root).unwrap().to_string_lossy().into_owned();
                let len = e.metadata()?.len();
                out.push((rel, len));
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.resolve(path)?)?)
    }

    pub fn write(&self, path: &str, bytes: &[u8]) -> Result<u64> {
        let p = self.resolve(path)?;
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, bytes)?;
        Ok(bytes.len() as u64)
    }

    pub fn delete(&self, path: &str) -> Result<()> {
        Ok(std::fs::remove_file(self.resolve(path)?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vfs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn roundtrip_and_list() {
        let dir = tmp("roundtrip");
        let v = Vfs::open(&dir).unwrap();
        v.write("up/report.txt", b"hi").unwrap();
        assert_eq!(v.read("up/report.txt").unwrap(), b"hi");
        let l = v.list("up").unwrap();
        assert_eq!(l.len(), 1);
        v.delete("up/report.txt").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn traversal_rejected() {
        let dir = tmp("traversal");
        let v = Vfs::open(&dir).unwrap();
        assert!(v.read("../../etc/passwd").is_err());
        assert!(v.read("/abs/path").is_err());
        assert!(v.write("a/../../../../x", b"y").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    #[test]
    fn empty_dir_lists_empty_and_missing_errors() {
        let dir = std::env::temp_dir().join(format!("vfs-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let v = Vfs::open(&dir).unwrap();
        assert!(v.list("").unwrap().is_empty());
        assert!(v.read("nope.txt").is_err());
        assert!(v.delete("nope.txt").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn curdir_segments_resolve_inside_root() {
        let dir = std::env::temp_dir().join(format!("vfs-dot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let v = Vfs::open(&dir).unwrap();
        v.write("a/./b.txt", b"dot").unwrap();
        assert_eq!(v.read("a/b.txt").unwrap(), b"dot");
        let l = v.list("").unwrap();
        assert_eq!(l.len(), 1);
        assert!(l[0].0.contains('a'));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_reports_byte_len() {
        let dir = std::env::temp_dir().join(format!("vfs-len-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let v = Vfs::open(&dir).unwrap();
        assert_eq!(v.write("f.bin", b"12345").unwrap(), 5);
        assert_eq!(v.list("").unwrap()[0].1, 5);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
