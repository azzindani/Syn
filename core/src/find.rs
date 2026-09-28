//! Finding a file by name, for a model that was told "open the budget" and
//! not where the budget lives.
//!
//! `open` wants a full path, and a person asking for "last month's sales
//! deck" does not say one. Without this the model's only moves were to
//! guess a path (and be told there is no such file) or to ask the person,
//! which is the step the console exists to take off them.
//!
//! A plain walk of the folders people keep documents in, bounded three
//! ways -- entries visited, depth, and wall time -- so a query that matches
//! nothing on a large disk still answers in seconds, and says it stopped
//! early rather than claiming the file is not there. Nothing here opens,
//! reads or runs a file: it looks at names, sizes and dates. Names are
//! still data from the disk, and the loop fences them like any result.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

const MAX_QUERY: usize = 200;
const MAX_FOLDER: usize = 1000;
/// Entries looked at before giving up. A user profile with a synced cloud
/// folder is typically tens of thousands; this covers it with room.
const MAX_VISITS: usize = 200_000;
const MAX_DEPTH: usize = 12;
const DEADLINE: Duration = Duration::from_secs(6);
/// Matches kept for sorting, and matches shown. Newest first is what a
/// person asking for "the report" nearly always means.
const MAX_KEPT: usize = 500;
const MAX_SHOWN: usize = 20;

/// Folders never worth descending into: somewhere else's files (the system,
/// installed programs, caches), or so many that they would spend the whole
/// budget (package trees, version control).
pub(crate) fn skipped(name: &str) -> bool {
    name.starts_with('.')
        || name.starts_with('$')
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "node_modules" | "appdata" | "__pycache__" | "windows" | "program files" | "program files (x86)" | "programdata"
                | "system volume information" | "recovery" | "msocache"
        )
}

/// What a name must look like to match: every word in the query, each on
/// its own terms.
///
/// A word with `*` or `?` is a wildcard over the whole name (`*.xlsx`,
/// `q?-report*`); any other word only has to appear somewhere in it. They
/// mix: the first live run searched `solar *.xlsx`, which as one wildcard
/// matched nothing, where a person means "solar, and a workbook".
struct Pattern(Vec<Term>);

enum Term {
    Glob(Vec<char>),
    Word(String),
}

impl Pattern {
    fn parse(query: &str) -> Self {
        Pattern(
            query
                .to_lowercase()
                .split_whitespace()
                .map(|w| if w.contains(['*', '?']) { Term::Glob(w.chars().collect()) } else { Term::Word(w.to_string()) })
                .collect(),
        )
    }

    fn matches(&self, name: &str) -> bool {
        let n = name.to_lowercase();
        let chars: Vec<char> = n.chars().collect();
        self.0.iter().all(|t| match t {
            Term::Word(w) => n.contains(w.as_str()),
            Term::Glob(p) => glob(p, &chars),
        })
    }
}

/// Wildcard match, iterative with one backtrack point: linear in practice,
/// and no recursion for a hostile pattern of many stars to blow up.
fn glob(p: &[char], s: &[char]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

pub(crate) fn home() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_dir())
}

/// Syn's own source checkout, when it runs from one: the folder holding
/// `core/Cargo.toml` and `widget/index.html`, found above the program or
/// the folder it started in.
///
/// Never searched unless named. It is full of test fixtures -- sample
/// workbooks, memos and decks, and a 52 MB CSV of the capability test's
/// data -- and `scripts/console.ps1` starts the console in it, so "the
/// folder Syn runs in" was the checkout. Reported from use: runs kept
/// searching for and opening the capability test's solar files, which the
/// person had never mentioned, because a search for "csv" or "xlsx" found
/// them and handed back an `open` call to copy.
pub(crate) fn own_tree() -> Option<PathBuf> {
    let mut from = Vec::new();
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        from.push(d);
    }
    if let Ok(c) = std::env::current_dir() {
        from.push(c);
    }
    from.iter()
        .flat_map(|s| s.ancestors())
        .find(|a| a.join("core").join("Cargo.toml").is_file() && a.join("widget").join("index.html").is_file())
        .and_then(|a| std::fs::canonicalize(a).ok())
}

/// Where people keep documents, when no folder is named: the usual three
/// under the profile, the synced cloud folder, and the folder Syn was
/// started in, unless that is Syn's own checkout (`own_tree`). A start
/// inside another start is dropped, so a Documents folder redirected into
/// OneDrive is walked once.
pub(crate) fn usual_places() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(h) = home() {
        for sub in ["Desktop", "Documents", "Downloads"] {
            out.push(h.join(sub));
        }
        if let Ok(rd) = std::fs::read_dir(&h) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("OneDrive") {
                    out.push(e.path());
                }
            }
        }
    }
    if let Some(od) = std::env::var_os("OneDrive") {
        out.push(PathBuf::from(od));
    }
    if let Ok(c) = std::env::current_dir() {
        let own = own_tree();
        let inside = std::fs::canonicalize(&c).ok().zip(own).is_some_and(|(c, o)| c.starts_with(o));
        if !inside {
            out.push(c);
        }
    }
    distinct(out)
}

/// Existing directories, canonical, with any that sit inside another
/// dropped.
fn distinct(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut canon: Vec<PathBuf> = dirs.into_iter().filter(|d| d.is_dir()).filter_map(|d| std::fs::canonicalize(d).ok()).collect();
    canon.sort();
    canon.dedup();
    let all = canon.clone();
    canon.retain(|d| !all.iter().any(|o| o != d && d.starts_with(o)));
    canon
}

struct Hit {
    path: PathBuf,
    size: u64,
    modified: u64,
}

/// Search, and say what was found as text for the model.
///
/// `roots`, when not empty, confine the search the way they confine `open`
/// (AGENT_MCP_ROOTS): a folder outside them is refused, and with no folder
/// named the roots themselves are searched.
pub fn search(query: &str, folder: Option<&str>, roots: &[PathBuf]) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("search: give part of the file's name, e.g. search{\"name\":\"budget xlsx\"}".into());
    }
    if q.chars().count() > MAX_QUERY || q.chars().any(char::is_control) {
        return Err(format!("search: the name is over {MAX_QUERY} characters or holds control characters; give a few words of it"));
    }
    if q.contains(['/', '\\']) {
        return Err("search: `name` is part of a file NAME; put the folder in `folder`".into());
    }
    let roots: Vec<PathBuf> = roots.iter().filter_map(|r| std::fs::canonicalize(r).ok()).collect();
    let starts = match folder.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => {
            if f.chars().count() > MAX_FOLDER {
                return Err(format!("search: folder is over {MAX_FOLDER} characters"));
            }
            let p = std::fs::canonicalize(f).map_err(|_| format!("search: no such folder: {f}"))?;
            if !p.is_dir() {
                return Err(format!("search: {f} is a file, not a folder; put its name in `name` and its folder in `folder`"));
            }
            if !roots.is_empty() && !roots.iter().any(|r| p.starts_with(r)) {
                return Err(format!(
                    "search: {f} is outside the folders this session may reach ({})",
                    roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ")
                ));
            }
            vec![p]
        }
        None if !roots.is_empty() => distinct(roots.clone()),
        None => usual_places(),
    };
    if starts.is_empty() {
        return Err("search: none of the usual folders exist here; name one with `folder`".into());
    }

    let pattern = Pattern::parse(q);
    // A checkout kept under Documents is still skipped on the way past; one
    // the model named, or a start inside it, is searched as asked.
    let own = own_tree().filter(|o| !starts.iter().any(|s| s.starts_with(o)));
    let (hits, stopped) = walk(&starts, &pattern, own.as_deref(), Instant::now() + DEADLINE);
    Ok(report(q, &starts, hits, stopped))
}

/// Breadth-first, so the shallow files people mean are found before the
/// budget goes on something buried. Symbolic links and junctions are not
/// followed: they loop, and they lead out of the folders searched.
fn walk(starts: &[PathBuf], pattern: &Pattern, skip: Option<&Path>, deadline: Instant) -> (Vec<Hit>, Option<&'static str>) {
    let mut hits = Vec::new();
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = starts.iter().map(|s| (s.clone(), 0)).collect();
    let mut visits = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            visits += 1;
            if visits >= MAX_VISITS {
                return (hits, Some("looked at the most entries one search may"));
            }
            if visits.is_multiple_of(512) && Instant::now() > deadline {
                return (hits, Some("ran out of time"));
            }
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().to_string();
            if ft.is_dir() {
                if depth + 1 < MAX_DEPTH && !skipped(&name) && skip.is_none_or(|o| !same_dir(&e.path(), o)) {
                    queue.push_back((e.path(), depth + 1));
                }
            } else if ft.is_file() && !name.starts_with("~$") && pattern.matches(&name) && hits.len() < MAX_KEPT {
                // `~$budget.xlsx` is Office's lock file for an open
                // budget.xlsx: never the file anyone means.
                let md = e.metadata().ok();
                let modified = md
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                hits.push(Hit { path: e.path(), size: md.map(|m| m.len()).unwrap_or(0), modified });
            }
        }
    }
    (hits, None)
}

/// Whether `dir` is `canonical`, comparing the cheap way first: only a
/// folder with the same name is worth resolving.
fn same_dir(dir: &Path, canonical: &Path) -> bool {
    dir.file_name() == canonical.file_name() && std::fs::canonicalize(dir).is_ok_and(|d| d == canonical)
}

fn report(q: &str, starts: &[PathBuf], mut hits: Vec<Hit>, stopped: Option<&str>) -> String {
    hits.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)));
    let places = starts.iter().map(|s| display(s)).collect::<Vec<_>>().join(", ");
    let mut out = String::new();
    if hits.is_empty() {
        out.push_str(&format!("No file matching {q:?} under {places}."));
        if let Some(why) = stopped {
            out.push_str(&format!(" The search stopped early ({why}), so it may still be there: name a narrower folder."));
        }
        out.push_str(
            "\nTry fewer words, a wildcard such as *.xlsx, or another folder: search{\"name\":\"...\",\"folder\":\"D:\\\\\"}. If it still is not found, ask where it is.",
        );
        return out;
    }
    let total = hits.len();
    out.push_str(&format!(
        "{} file{} matching {q:?}, newest first{}:\n",
        if total >= MAX_KEPT { format!("{total}+") } else { total.to_string() },
        if total == 1 { "" } else { "s" },
        if total > MAX_SHOWN { format!(" (the first {MAX_SHOWN})") } else { String::new() }
    ));
    for h in hits.iter().take(MAX_SHOWN) {
        out.push_str(&format!("- {}  ({}, modified {})\n", display(&h.path), size(h.size), date(h.modified)));
    }
    out.push_str(&format!("Searched {places}."));
    if let Some(why) = stopped {
        out.push_str(&format!(" Stopped early ({why}): a file deeper down may be missing from this list."));
    }
    // The call to copy, when exactly one file here is one an app opens.
    // With several, newest is a guess: the newest CSV on a disk is rarely
    // "the CSV" a person means, and a model handed a call to copy opens it.
    let openable: Vec<(&Hit, &str)> = hits
        .iter()
        .filter_map(|h| {
            let name = h.path.file_name()?.to_string_lossy().to_string();
            crate::desk::app_for_file(&name).map(|a| (h, a))
        })
        .collect();
    match openable.as_slice() {
        [] => {}
        [(h, app)] => {
            let args = crate::json::obj(vec![("app", crate::json::s(*app)), ("path", crate::json::s(display(&h.path)))]);
            out.push_str(&format!("\nNext: open{} if that is the one.", args.to_json()));
        }
        many => out.push_str(&format!(
            "\n{} of these could be opened. Open the one the user's words pick out; if they do not pick out one, ask which, rather than taking the newest.",
            many.len()
        )),
    }
    out
}

/// A path as a person would type it. `canonicalize` on Windows returns the
/// verbatim form, `\\?\C:\...`, which is correct and unreadable, and which
/// a model then copies into `open`.
pub(crate) fn display(p: &Path) -> String {
    let s = p.display().to_string();
    match s.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s),
    }
}

pub(crate) fn size(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{} KB", n >> 10),
        n => format!("{n} bytes"),
    }
}

/// `YYYY-MM-DD` from seconds since the epoch, in UTC. Howard Hinnant's
/// civil-from-days, because the crate takes no date library.
pub(crate) fn date(secs: u64) -> String {
    if secs == 0 {
        return "unknown".into();
    }
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder tree of its own under the system temp dir, removed after.
    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str, files: &[&str]) -> Self {
            let root = std::env::temp_dir().join(format!("syn-find-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for f in files {
                let p = root.join(f);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, b"x").unwrap();
            }
            Tree(root)
        }
        fn at(&self) -> String {
            self.0.display().to_string()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn every_word_must_be_in_the_name_in_any_order() {
        let t = Tree::new("words", &["q3/Solar Deck final.pptx", "solar.xlsx", "notes.txt"]);
        let got = search("deck solar", Some(&t.at()), &[]).unwrap();
        assert!(got.contains("Solar Deck final.pptx"), "{got}");
        assert!(!got.contains("solar.xlsx"), "{got}");
        assert!(got.starts_with("1 file matching"), "{got}");
    }

    #[test]
    fn a_wildcard_matches_the_whole_name() {
        let t = Tree::new("glob", &["a/plan.xlsx", "plan.xlsx.bak", "b/c/budget.XLSX", "readme.md"]);
        let got = search("*.xlsx", Some(&t.at()), &[]).unwrap();
        assert!(got.contains("plan.xlsx") && got.contains("budget.XLSX"), "{got}");
        assert!(!got.contains("plan.xlsx.bak") && !got.contains("readme"), "{got}");
        assert!(glob(&['a', '*', 'c'], &['a', 'b', 'b', 'c']));
        assert!(!glob(&['a', '?', 'c'], &['a', 'c']));
    }

    #[test]
    fn a_word_and_a_wildcard_mix_the_way_a_model_writes_them() {
        // Verbatim from the first live run: `solar *.xlsx` found nothing
        // while solar.xlsx sat in the folder searched.
        let t = Tree::new("mixed", &["docs/solar.xlsx", "docs/solar-memo.docx", "docs/plan.xlsx"]);
        let got = search("solar *.xlsx", Some(&t.at()), &[]).unwrap();
        assert!(got.starts_with("1 file matching"), "{got}");
        assert!(got.contains("solar.xlsx"), "{got}");
    }

    #[test]
    fn a_found_office_file_comes_with_the_open_call_to_copy() {
        let t = Tree::new("next", &["deep/er/report.docx"]);
        let got = search("report", Some(&t.at()), &[]).unwrap();
        let next = got.lines().find(|l| l.starts_with("Next: open")).expect(&got);
        let json = next.trim_start_matches("Next: open").trim_end_matches(" if that is the one.");
        let v = crate::json::parse(json).expect(json);
        assert_eq!(v.get("app").and_then(crate::json::Value::as_str), Some("word"));
        let path = v.get("path").and_then(crate::json::Value::as_str).unwrap();
        assert!(Path::new(path).is_file(), "the path given must be one `open` can use: {path}");
        assert!(!path.starts_with(r"\\?\"), "no verbatim prefix for a model to copy: {path}");
    }

    #[test]
    fn several_openable_matches_are_listed_and_the_model_is_told_to_ask_not_guess() {
        // Handed an `open` call for the newest of several, a model opens it:
        // the newest CSV on a disk is rarely "the CSV" a person means.
        let t = Tree::new("many", &["a/sales.csv", "b/sales-2025.csv"]);
        let got = search("sales", Some(&t.at()), &[]).unwrap();
        assert!(got.contains("sales.csv") && got.contains("sales-2025.csv"), "{got}");
        assert!(!got.contains("Next: open"), "no call to copy when it would be a guess: {got}");
        assert!(got.contains("ask which"), "{got}");
    }

    #[test]
    fn syn_own_checkout_is_not_one_of_the_usual_places() {
        // `cargo test` runs in core/, inside the checkout, just as the
        // console does: the folder it starts in must not be searched, or
        // the capability fixtures come back for every "csv" or "xlsx".
        let own = own_tree().expect("the tests run inside the checkout");
        assert!(own.join("core").join("Cargo.toml").is_file());
        for p in usual_places() {
            assert!(!p.starts_with(&own), "{} is inside Syn's own checkout", p.display());
        }
    }

    #[test]
    fn a_checkout_below_a_searched_folder_is_walked_past() {
        let t = Tree::new("own", &["Syn/core/Cargo.toml", "Syn/widget/index.html", "Syn/testbed/budget.xlsx", "work/budget.xlsx"]);
        let own = std::fs::canonicalize(t.0.join("Syn")).unwrap();
        let starts = vec![std::fs::canonicalize(&t.0).unwrap()];
        let (hits, _) = walk(&starts, &Pattern::parse("budget"), Some(&own), Instant::now() + DEADLINE);
        let found: Vec<String> = hits.iter().map(|h| h.path.display().to_string()).collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("work"), "{found:?}");
    }

    #[test]
    fn hidden_folders_package_trees_and_office_lock_files_are_not_searched() {
        let t = Tree::new("skip", &[".git/plan.xlsx", "node_modules/x/plan.xlsx", "~$plan.xlsx", "real/plan.xlsx"]);
        let got = search("plan", Some(&t.at()), &[]).unwrap();
        assert!(got.starts_with("1 file matching"), "{got}");
        assert!(got.contains("real"), "{got}");
    }

    #[test]
    fn nothing_found_says_what_to_try_rather_than_guessing() {
        let t = Tree::new("none", &["a.txt"]);
        let got = search("quarterly", Some(&t.at()), &[]).unwrap();
        assert!(got.starts_with("No file matching"), "{got}");
        assert!(got.contains("search{"), "{got}");
    }

    #[test]
    fn a_bad_call_is_refused_with_how_to_fix_it() {
        assert!(search("  ", None, &[]).unwrap_err().contains("part of the file's name"));
        assert!(search(r"C:\x\plan.xlsx", None, &[]).unwrap_err().contains("folder"));
        assert!(search(&"x".repeat(MAX_QUERY + 1), None, &[]).is_err());
        assert!(search("plan", Some(r"Z:\surely\not\here"), &[]).unwrap_err().contains("no such folder"));
    }

    #[test]
    fn roots_confine_the_search_as_they_confine_open() {
        let inside = Tree::new("root-in", &["plan.xlsx"]);
        let outside = Tree::new("root-out", &["plan.xlsx"]);
        let roots = vec![inside.0.clone()];
        assert!(search("plan", Some(&outside.at()), &roots).unwrap_err().contains("outside"));
        // No folder named: the roots are where it looks.
        let got = search("plan", None, &roots).unwrap();
        assert!(got.contains("root-in") && !got.contains("root-out"), "{got}");
    }

    #[test]
    fn dates_are_calendar_dates() {
        assert_eq!(date(0), "unknown");
        assert_eq!(date(86_400), "1970-01-02");
        assert_eq!(date(1_709_164_800), "2024-02-29");
        assert_eq!(date(1_790_000_000), "2026-09-21");
    }
}
