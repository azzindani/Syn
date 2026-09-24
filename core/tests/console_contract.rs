//! The console renders what the loop emits, and this checks that the two
//! agree — with no browser, no Node and no network.
//!
//! Why it exists: the console reads the CLI's own output. That makes the
//! wording of a log line part of an interface, which is exactly the kind
//! of coupling nobody remembers. Reworded `STEP {tool}: {detail}` to
//! `STEP {sentence}` while improving the terminal output and the live view
//! silently stopped drawing tool rows — no error anywhere, the page just
//! went quiet during a run. That is the failure this catches.
//!
//! The fix that made it checkable was to split the two audiences: `STEP`,
//! `REFUSED` and `DID` are prose for a person, and `RECEIPT <kind> {json}`
//! is the same event as data, which is the only thing the page parses.
//! This test holds that split in place from both directions.
//!
//! What it does NOT check: that the page looks right, or that the rows are
//! in the right order. That is what `tests/ui` drives a real browser for.

use std::fs;
use std::path::PathBuf;

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

/// Read a source file with its line endings normalised. `core.autocrlf` is
/// true here, so the same commit is CRLF on Windows and LF on Linux and a
/// check that depends on either passes on two machines and fails on the
/// third.
fn source(rel: &str) -> String {
    fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}")).replace('\r', "")
}

fn cli() -> String {
    source("core/src/bin/cli.rs")
}

fn page() -> String {
    source("widget/index.html")
}

/// Every `RECEIPT <kind>` the CLI can print.
fn emitted() -> Vec<String> {
    let src = cli();
    let mut out = Vec::new();
    // A string literal opening `"RECEIPT `. That catches both a direct
    // `pr!("RECEIPT x ...")` and one built with `format!` and printed
    // later, while a comment mentioning a receipt has no quote in front
    // of it and is not counted.
    let needle = "\"RECEIPT ";
    for (i, _) in src.match_indices(needle) {
        let rest = &src[i + needle.len()..];
        let kind: String = rest.chars().take_while(|c| c.is_ascii_lowercase()).collect();
        if !kind.is_empty() && !out.contains(&kind) {
            out.push(kind);
        }
    }
    out.sort();
    out
}

/// Every `RECEIPT <kind>` the page has a branch for.
fn rendered() -> Vec<String> {
    let src = page();
    let mut out = Vec::new();
    // Only inside a regex: `/^RECEIPT <kind>`. A comment mentioning one is
    // not a branch that draws it.
    let needle = "^RECEIPT ";
    for (i, _) in src.match_indices(needle) {
        let rest = &src[i + needle.len()..];
        let kind: String = rest.chars().take_while(|c| c.is_ascii_lowercase()).collect();
        if !kind.is_empty() && !out.contains(&kind) {
            out.push(kind);
        }
    }
    out.sort();
    out
}

/// Receipts that are a reply to a command the page sent, not something to
/// draw during a run. The page reads these out of the `/cmd` response, so
/// there is no live-tail branch for them and there should not be one.
const COMMAND_REPLIES: &[&str] = &[
    // State the page asks for and reads out of the /cmd reply.
    "waiting", "env", "mark", "session", "attached", "slots", "wiring", "registry", "events", "config", "task",
    "shell", "do", "chat", "hands", "hand", "route", "live", "open", "allowlist", "deleted",
    // Acknowledgements of a control the human just pressed. The page
    // already knows it pressed it; redrawing on the echo would fight the
    // optimistic update.
    "killed", "paused", "resumed", "send", "pump", "line",
    // Journal and replay: a developer path driven from the terminal, with
    // no console surface at all.
    "journal", "replay",
];

#[test]
fn every_receipt_the_run_emits_has_somewhere_to_land() {
    let rendered = rendered();
    let missing: Vec<String> = emitted()
        .into_iter()
        .filter(|k| !COMMAND_REPLIES.contains(&k.as_str()))
        .filter(|k| !rendered.contains(k))
        .collect();
    assert!(
        missing.is_empty(),
        "the CLI emits RECEIPT {missing:?} during a run and the console draws nothing for them.\n\
         Either add a branch in widget/index.html or, if it is a reply to a command rather than \
         something to watch, list it in COMMAND_REPLIES with a reason."
    );
}

#[test]
fn the_console_draws_nothing_the_cli_never_sends() {
    let emitted = emitted();
    let dead: Vec<String> = rendered().into_iter().filter(|k| !emitted.contains(k)).collect();
    assert!(dead.is_empty(), "the console has a branch for RECEIPT {dead:?}, which nothing emits any more");
}

#[test]
fn the_live_view_parses_data_and_not_prose() {
    let page = page();
    // The regression: a page that matched on `STEP <tool>: <detail>` broke
    // the moment that line was reworded. The prose lines must be skipped
    // explicitly, so the skip is visible rather than accidental.
    assert!(
        page.contains("/^(STEP|REFUSED|DID) /"),
        "the page must explicitly ignore the human-facing lines, so that rewording one cannot \
         quietly change what is drawn"
    );
    for prose in ["^STEP (\\S+?):", "^REFUSED (.*)$"] {
        assert!(!page.contains(prose), "the page still scrapes the prose line {prose:?}");
    }
}

#[test]
fn the_page_never_builds_a_tool_label_itself() {
    // Labels come from `core::labels`, over the wire, as `LABEL` lines and
    // `RECEIPT step`. A second table in JavaScript would drift from the
    // tool surface the moment a verb was added, and would sit outside the
    // Rust test that stops a new verb reaching a human as raw JSON.
    let page = page();
    for verb in ["insertParagraph", "addSheet", "createSlide", "conditional", "pageNumbers"] {
        assert!(!page.contains(verb), "widget/index.html names the {verb:?} verb: labels belong in core");
    }
    assert!(page.contains("labels[c.id]"), "the saved transcript should render from the LABEL lines");
}

#[test]
fn the_cli_sends_a_label_and_a_status_for_every_call() {
    let cli = cli();
    assert!(cli.contains("LABEL "), "the transcript is sent with no per-call labels");
    assert!(cli.contains("RECEIPT step "), "a running turn sends no per-call receipts");
    assert!(cli.contains("RECEIPT did "), "a finished run sends no summary");
    // The status vocabulary is shared, and the page keys its colours off
    // it. Both sides have to spell it the same way.
    let page = page();
    for status in ["refused", "failed", "stopped"] {
        assert!(cli.contains(&format!("\"{status}\"")), "the CLI never sends status {status:?}");
        assert!(page.contains(&format!("data-status=\"{status}\"")), "the page has no style for {status:?}");
    }
}
