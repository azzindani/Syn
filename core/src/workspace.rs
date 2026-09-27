//! A workspace: the one folder a chat works in.
//!
//! Without one, `search` walked Desktop, Documents, Downloads, OneDrive and
//! the folder Syn started in, and "the budget" matched three old copies as
//! readily as the one meant. A person working on one job already knows
//! where it lives. Picking that folder does three things: `search` looks
//! only there, `open` refuses anything outside it (the confinement MCP
//! already had, now reachable from the chat), and the model is shown what
//! documents the folder holds before it has asked, so "open the sales
//! workbook" needs no search at all.
//!
//! The listing is names, sizes and dates, never contents. Reading contents
//! would mean opening every file in its application, slowly, and pouring
//! untrusted text into the model before anything needed it; and this
//! project does not read Office files except through the applications.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::find::{date, display, size, skipped, usual_places};

/// Longest path accepted from the page, the same cap `search` puts on a
/// folder.
pub const MAX_PATH: usize = 1000;

/// The documents a listing names: what the apps Syn drives can open, and
/// the flat files people keep beside them.
const DOCS: &[&str] = &[
    "xlsx", "xlsm", "xlsb", "xls", "csv", "docx", "docm", "doc", "rtf", "pptx", "pptm", "ppt", "pdf", "txt", "md",
];

/// How much of the folder the listing looks at: a workspace set to a whole
/// drive must still answer at once.
const LIST_VISITS: usize = 5_000;
const LIST_DEPTH: usize = 4;
const LIST_TIME: Duration = Duration::from_millis(800);
const LIST_SHOWN: usize = 30;

/// Subfolders shown per level in the picker.
const DIRS_SHOWN: usize = 200;

/// The folder a path names, checked and made absolute, or why not.
pub fn resolve(path: &str) -> Result<PathBuf, String> {
    let p = path.trim().trim_matches('"').trim();
    if p.is_empty() {
        return Err("workspace: name a folder, or `workspace off` for everywhere".into());
    }
    if p.chars().count() > MAX_PATH {
        return Err(format!("workspace: the path is over {MAX_PATH} characters"));
    }
    let pb = PathBuf::from(p);
    if !pb.is_absolute() {
        return Err(format!("workspace: {p:?} is not a full path; give one such as D:\\Projects\\Q3"));
    }
    if !pb.is_dir() {
        return Err(format!("workspace: {p} is not a folder that exists"));
    }
    std::fs::canonicalize(&pb).map_err(|e| format!("workspace: {p}: {e}"))
}

/// A folder as a person reads it: no `\\?\` prefix.
pub fn shown(p: &Path) -> String {
    display(p)
}

/// What the model is told about the workspace: a line saying what it is and
/// what it changes, and the documents in it, newest first. The listing is
/// names from the disk, so the caller fences it as untrusted.
pub struct Note {
    pub head: String,
    pub listing: String,
}

pub fn note(root: &Path) -> Note {
    let head = format!(
        "Workspace: {}. `search` looks only in this folder and `open` refuses files outside it. \
         Inside it, `open` also takes a name or a path relative to it, as listed below.",
        shown(root)
    );
    let (mut docs, others, cut) = documents(root);
    docs.sort_by_key(|d| std::cmp::Reverse(d.2));
    let total = docs.len();
    let mut listing = if total == 0 {
        "No documents here yet (Office files, PDF, CSV or text).".to_string()
    } else if total > LIST_SHOWN {
        format!("Documents here, newest first ({LIST_SHOWN} of {total}; `search` finds the rest):\n")
    } else {
        format!("Documents here, newest first ({total}):\n")
    };
    for (rel, bytes, modified) in docs.iter().take(LIST_SHOWN) {
        listing.push_str(&format!("- {rel} ({}, {})\n", size(*bytes), date(*modified)));
    }
    if others > 0 {
        listing.push_str(&format!("{others} other file(s) are not documents and are not listed.\n"));
    }
    if cut {
        listing.push_str("The folder is large and was not looked through to the end.\n");
    }
    Note { head, listing: listing.trim_end().to_string() }
}

/// Documents under `root` as (path relative to it, size, modified), the
/// count of other files, and whether the walk stopped early.
fn documents(root: &Path) -> (Vec<(String, u64, u64)>, usize, bool) {
    let deadline = Instant::now() + LIST_TIME;
    let mut out = Vec::new();
    let mut others = 0;
    let mut visits = 0;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            visits += 1;
            if visits > LIST_VISITS || Instant::now() > deadline {
                return (out, others, true);
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth + 1 < LIST_DEPTH && !skipped(&name) {
                    stack.push((e.path(), depth + 1));
                }
                continue;
            }
            // Office's own lock files, `~$budget.xlsx`, are not documents.
            let ext = name.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase()).unwrap_or_default();
            if name.starts_with("~$") || !DOCS.contains(&ext.as_str()) {
                others += 1;
                continue;
            }
            let meta = e.metadata().ok();
            let bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let rel = e.path().strip_prefix(root).map(|r| r.display().to_string()).unwrap_or(name);
            out.push((rel, bytes, modified));
        }
    }
    (out, others, false)
}

/// One folder the picker can show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dir {
    pub name: String,
    pub path: String,
}

/// The folders to choose from: the subfolders of `path`, or with no path
/// the places people start from -- their usual document folders and, on
/// Windows, every drive.
///
/// This is the one thing the page cannot do itself: a browser will not
/// hand a web page the real path of a folder, so the list comes from here.
pub fn dirs(path: Option<&str>) -> Result<Vec<Dir>, String> {
    let Some(p) = path.map(str::trim).filter(|p| !p.is_empty()) else {
        let mut out: Vec<Dir> = usual_places()
            .into_iter()
            .map(|d| Dir { name: d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), path: shown(&d) })
            .collect();
        if cfg!(windows) {
            for letter in b'A'..=b'Z' {
                let root = format!("{}:\\", letter as char);
                if Path::new(&root).is_dir() {
                    out.push(Dir { name: root.clone(), path: root });
                }
            }
        } else {
            out.push(Dir { name: "/".into(), path: "/".into() });
        }
        return Ok(out);
    };
    let root = resolve(p)?;
    let rd = std::fs::read_dir(&root).map_err(|e| format!("dirs: {}: {e}", shown(&root)))?;
    let mut out: Vec<Dir> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !skipped(n))
        .map(|n| Dir { path: shown(&root.join(&n)), name: n })
        .collect();
    out.sort_by_key(|d| d.name.to_lowercase());
    out.truncate(DIRS_SHOWN);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);
    impl Tree {
        fn new(tag: &str, files: &[&str]) -> Self {
            let root = std::env::temp_dir().join(format!("syn-ws-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for f in files {
                let p = root.join(f);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, b"x").unwrap();
            }
            std::fs::create_dir_all(&root).unwrap();
            Tree(std::fs::canonicalize(&root).unwrap())
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_workspace_must_be_a_folder_that_exists_named_in_full() {
        assert!(resolve("").unwrap_err().contains("workspace off"));
        assert!(resolve("reports").unwrap_err().contains("not a full path"));
        let t = Tree::new("resolve", &["a.xlsx"]);
        assert!(resolve(&t.0.join("a.xlsx").display().to_string()).unwrap_err().contains("not a folder"));
        assert!(resolve(&t.0.join("gone").display().to_string()).unwrap_err().contains("not a folder"));
        assert_eq!(resolve(&format!("\"{}\"", t.0.display())).unwrap(), t.0, "quotes from a pasted path are dropped");
    }

    #[test]
    fn the_listing_names_documents_by_their_path_in_the_workspace_and_counts_the_rest() {
        let t = Tree::new(
            "list",
            &["plan.xlsx", "q3/deck.pptx", "q3/notes.txt", "q3/~$deck.pptx", "tool.exe", "node_modules/x.docx", ".git/y.docx"],
        );
        let n = note(&t.0);
        assert!(n.head.contains(&shown(&t.0)), "{}", n.head);
        assert!(n.listing.contains("- plan.xlsx ("), "{}", n.listing);
        assert!(n.listing.contains(&format!("- {} (", Path::new("q3").join("deck.pptx").display())), "{}", n.listing);
        assert!(n.listing.contains("Documents here, newest first (3)"), "{}", n.listing);
        assert!(!n.listing.contains("~$"), "lock files are not documents: {}", n.listing);
        assert!(!n.listing.contains("x.docx") && !n.listing.contains("y.docx"), "skipped folders stay skipped: {}", n.listing);
        assert!(n.listing.contains("2 other file(s)"), "{}", n.listing);
    }

    #[test]
    fn an_empty_workspace_says_so() {
        let t = Tree::new("empty", &[]);
        assert!(note(&t.0).listing.starts_with("No documents here yet"));
    }

    #[test]
    fn the_picker_lists_subfolders_and_starts_from_the_usual_places() {
        let t = Tree::new("dirs", &["b/one.txt", "a/two.txt", "node_modules/z.txt", "loose.txt"]);
        let got = dirs(Some(&t.0.display().to_string())).unwrap();
        let names: Vec<&str> = got.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["a", "b"], "folders only, sorted, skipped ones hidden");
        assert_eq!(got[0].path, shown(&t.0.join("a")));
        let start = dirs(None).unwrap();
        assert!(!start.is_empty(), "somewhere to start from");
        if cfg!(windows) {
            assert!(start.iter().any(|d| d.path.len() == 3 && d.path.ends_with(":\\")), "{start:?}");
        }
    }
}
