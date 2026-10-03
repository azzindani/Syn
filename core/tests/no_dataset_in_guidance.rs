//! Nothing the model is shown may name a dataset the tests use.
//!
//! The older ban lists (`manual.rs`, `tools.rs`) hold the words of one
//! fixture, the Calgary solar file, and nothing else: a new dataset got no
//! protection, so guidance written while watching a run on it could name
//! its columns and no test would say so. A manual, a tool description or an
//! example that names a column from `samples/` teaches the test and not the
//! tool, and a model that copies it has shown nothing about its own work.
//!
//! The words come from `samples/` itself: every file's name, and every
//! column header that could only be that dataset's (a compound like
//! `lead_time` or `originTypeName`, not a plain word like `country` that
//! documentation uses for its own reasons). Adding a dataset to `samples/`
//! extends the ban with no edit here.

use std::collections::BTreeSet;
use std::path::Path;

/// Subject words no tool documentation has a reason to use. Not derivable
/// from a header, so named here; a new dataset's subject goes in this list.
const SUBJECTS: &[&str] = &["hotel", "booking", "crude oil", "electricity production", "calgary", "bearspaw"];

fn dataset_words() -> BTreeSet<String> {
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("samples");
    let mut words = BTreeSet::new();
    for s in SUBJECTS {
        words.insert(s.to_string());
    }
    let dir = std::fs::read_dir(&samples).unwrap_or_else(|e| panic!("{}: {e}", samples.display()));
    for entry in dir.flatten() {
        let path = entry.path();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        if !matches!(ext.as_str(), "csv" | "zip" | "xlsx") {
            continue;
        }
        words.insert(stem.to_ascii_lowercase());
        if ext != "csv" {
            continue;
        }
        let text = std::fs::read(&path).unwrap_or_default();
        let first = String::from_utf8_lossy(&text);
        let header = first.trim_start_matches('\u{feff}').lines().next().unwrap_or("");
        for col in header.split(',') {
            let col = col.trim().trim_matches('"');
            // A compound is one only this dataset names: snake_case, or a
            // camelCase boundary. A plain word is documentation's too.
            let camel = col.chars().skip(1).any(|c| c.is_ascii_uppercase()) && col.chars().any(|c| c.is_ascii_lowercase());
            if col.len() >= 6 && (col.contains('_') || camel) {
                words.insert(col.to_ascii_lowercase());
            }
        }
    }
    words
}

/// Everything the model is shown that is written by hand.
fn model_facing() -> Vec<(String, String)> {
    let mut out = vec![("the system prompt".to_string(), core::agent::SYSTEM.to_string())];
    for p in core::manual::PAGES {
        out.push((format!("manual page {:?}", p.topic), format!("{} {}", p.summary, p.body)));
    }
    for t in core::tools::TOOLS {
        out.push((format!("tool {:?}", t.name), format!("{} {} {}", t.name, t.description, t.params)));
    }
    for r in core::coach::RULES {
        out.push((format!("coaching for {:?}", r.needle), format!("{} {}", r.advice, r.mcp.unwrap_or(""))));
    }
    for app in ["excel", "word", "ppt", "powerpoint", "uia", "cdp"] {
        out.push((format!("selector help for {app}"), core::coach::selectors(app).to_string()));
    }
    out
}

#[test]
fn the_derived_ban_list_is_not_empty_and_knows_the_real_headers() {
    // A ban list that quietly came out empty (samples/ moved, a header
    // format changed) would pass every other test here.
    let w = dataset_words();
    for expect in ["hotel_bookings_demand", "lead_time", "is_canceled", "origintypename", "installationdate"] {
        assert!(w.contains(expect), "samples/ no longer yields {expect:?}: {w:?}");
    }
}

#[test]
fn nothing_the_model_is_shown_names_a_dataset_the_tests_use() {
    let words = dataset_words();
    let mut found = Vec::new();
    for (what, text) in model_facing() {
        let hay = text.to_ascii_lowercase();
        for w in &words {
            if hay.contains(w.as_str()) {
                found.push(format!("{what} mentions {w:?}"));
            }
        }
    }
    assert!(
        found.is_empty(),
        "guidance that names a dataset teaches the test, not the tool; use a placeholder:\n{}",
        found.join("\n")
    );
}
