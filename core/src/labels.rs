//! What a tool call is called when a human reads it.
//!
//! Ported from t3code's `work-log/presentation.ts`. The idea there is that a
//! tool's *name* never reaches the screen: a table maps each tool to four
//! grammatical forms, the lifecycle status picks one, and the arguments
//! supply a specific target when they have one. `link_pull_request({url})`
//! becomes "Linked PR #482", and while it runs, "Linking PR #482".
//!
//! Ours is the same shape over the six primitives. `struct` with
//! `verb: "chart"` is not shown as `struct{"verb":"chart",...}` but as
//! "Drew a chart on Dashboard". A run that makes forty `office-rpc/1`
//! envelopes reads as three sentences instead of forty lines.
//!
//! It lives in `core`, not in a renderer, for the reason `docs/09` gives:
//! a label describes a **world tool**, so it belongs with the vocabulary it
//! describes and travels to every client over the existing event feed. It is
//! also pure, which is why every line of it is tested without a document, a
//! model or a screen.

use crate::tools::{field, STRUCT_VERBS};

/// Where a call is in its life. The verb form follows from this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Dispatched, no result yet.
    Running,
    /// Finished, and the document changed or answered.
    Done,
    /// Reached the application and the application said no.
    Failed,
    /// Never reached the application: a bad call, or a gate.
    Refused,
    /// Cut off by a kill switch, a dead pipe or a spent budget.
    Stopped,
}

/// The four forms a tool is spoken in: bare action, present participle,
/// past tense, and the generic noun used when the arguments name nothing
/// specific.
type Forms = (&'static str, &'static str, &'static str, &'static str);

/// `read`, `write`, `format`, `export`, `undo`, `shell`, and the two loop
/// services. `struct` is not here: its verb decides, see [`STRUCT_FORMS`].
const TOOL_FORMS: &[(&str, Forms)] = &[
    ("read", ("Read", "Reading", "Read", "a range")),
    ("write", ("Write", "Writing", "Wrote", "cells")),
    ("format", ("Format", "Formatting", "Formatted", "cells")),
    ("export", ("Export", "Exporting", "Exported", "a document")),
    ("undo", ("Undo", "Undoing", "Undid", "the last change")),
    ("shell", ("Run", "Running", "Ran", "a program")),
    ("manual", ("Read", "Reading", "Read", "the manual")),
    ("plan", ("Update", "Updating", "Updated", "the plan")),
];

/// One row per `struct` verb. The table is checked against
/// [`crate::tools::STRUCT_VERBS`] by a test, so adding a verb without
/// giving it words fails the build rather than reaching a human as
/// `struct{"verb":"newThing"}`.
const STRUCT_FORMS: &[(&str, Forms)] = &[
    ("insertParagraph", ("Add", "Adding", "Added", "a paragraph")),
    ("insertTable", ("Add", "Adding", "Added", "a table")),
    ("addSheet", ("Add", "Adding", "Added", "a worksheet")),
    ("createSlide", ("Add", "Adding", "Added", "a slide")),
    ("transfer", ("Transfer", "Transferring", "Transferred", "data between documents")),
    ("invoke", ("Press", "Pressing", "Pressed", "a control")),
    ("pivot", ("Build", "Building", "Built", "a pivot table")),
    ("chart", ("Draw", "Drawing", "Drew", "a chart")),
    ("table", ("Create", "Creating", "Created", "a table")),
    ("name", ("Name", "Naming", "Named", "a range")),
    ("conditional", ("Shade", "Shading", "Shaded", "a range by its values")),
    ("slicer", ("Add", "Adding", "Added", "a slicer")),
    ("macro", ("Write", "Writing", "Wrote", "a macro")),
    ("pageBreak", ("Insert", "Inserting", "Inserted", "a page break")),
    ("contents", ("Insert", "Inserting", "Inserted", "a table of contents")),
    ("pageNumbers", ("Add", "Adding", "Added", "page numbers")),
    ("picture", ("Insert", "Inserting", "Inserted", "a picture")),
    ("find", ("Search", "Searching", "Searched", "the document")),
    ("replace", ("Replace", "Replacing", "Replaced", "text")),
    ("delete", ("Delete", "Deleting", "Deleted", "part of the document")),
    ("insert", ("Insert", "Inserting", "Inserted", "rows or columns")),
    ("sort", ("Sort", "Sorting", "Sorted", "a range")),
    ("filter", ("Filter", "Filtering", "Filtered", "a range")),
    ("dedupe", ("Remove", "Removing", "Removed", "duplicate rows")),
    ("copy", ("Copy", "Copying", "Copied", "a range")),
    ("validate", ("Add", "Adding", "Added", "a validation rule")),
    ("sheet", ("Change", "Changing", "Changed", "a worksheet")),
    ("comment", ("Add", "Adding", "Added", "a comment")),
    ("link", ("Add", "Adding", "Added", "a link")),
    ("pageSetup", ("Set up", "Setting up", "Set up", "the page")),
    ("header", ("Write", "Writing", "Wrote", "a header")),
    ("textBox", ("Add", "Adding", "Added", "a text box")),
    ("duplicateSlide", ("Duplicate", "Duplicating", "Duplicated", "a slide")),
    ("moveSlide", ("Move", "Moving", "Moved", "a slide")),
    ("theme", ("Apply", "Applying", "Applied", "a theme")),
];

/// `macro` is four different sentences depending on its `action`, because
/// writing code and running code are not the same event to someone watching.
const MACRO_FORMS: &[(&str, Forms)] = &[
    ("write", ("Write", "Writing", "Wrote", "a macro")),
    ("run", ("Run", "Running", "Ran", "a macro")),
    ("read", ("Read", "Reading", "Read", "a macro")),
    ("list", ("List", "Listing", "Listed", "the macros")),
];

fn lookup(table: &[(&str, Forms)], key: &str) -> Option<Forms> {
    table.iter().find(|(k, _)| *k == key).map(|(_, f)| *f)
}

/// The verb form for a status. Failure and refusal are phrased from the
/// bare action ("Failed to draw"), which is why the table carries one.
fn conjugate(forms: Forms, status: Status) -> String {
    let (action, running, done, _) = forms;
    match status {
        Status::Running => running.to_string(),
        Status::Done => done.to_string(),
        Status::Failed => format!("Failed to {}", lower_first(action)),
        Status::Refused => format!("Refused to {}", lower_first(action)),
        Status::Stopped => format!("Stopped {}", lower_first(running)),
    }
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Fold a call into `(tool, verb)`.
///
/// The surface accepts a `struct` verb as a tool name in its own right, and
/// `tools::to_action` folds it back into `struct` before dispatch. Every
/// function below has to agree with that, or a call sent as `chart{...}`
/// gets a generic label while the identical call sent as
/// `struct{"verb":"chart"}` gets a good one.
fn normalise(name: &str, args: &str) -> (&'static str, String) {
    if name == "struct" {
        return ("struct", field(args, "verb").unwrap_or_default());
    }
    if let Some(v) = STRUCT_VERBS.iter().find(|v| **v == name) {
        return ("struct", (*v).to_string());
    }
    // Not a struct verb: the tool is its own name, and there is no verb.
    let known = crate::tools::TOOLS.iter().find(|t| t.name == name).map(|t| t.name);
    let service = crate::looptools::SERVICES.iter().find(|s| s.name == name).map(|s| s.name);
    (known.or(service).unwrap_or(""), String::new())
}

/// The unit of a handle: the `Sheet1` in `excel:plan.xlsx:Sheet1`. Used as
/// a fallback target so a call that names no range still says where it
/// landed.
fn unit(handle: &str) -> Option<String> {
    let mut parts = handle.splitn(3, ':');
    let _app = parts.next()?;
    let _file = parts.next()?;
    parts.next().filter(|u| !u.is_empty()).map(str::to_string)
}

/// The file of a handle: the `plan.xlsx`.
fn file(handle: &str) -> Option<String> {
    let mut parts = handle.splitn(3, ':');
    let _app = parts.next()?;
    parts.next().filter(|f| !f.is_empty()).map(str::to_string)
}

/// The application a handle names, for an icon: `excel`, `word`, ...
pub fn app(args: &str) -> Option<String> {
    let handle = field(args, "handle")?;
    handle.split(':').next().filter(|a| !a.is_empty()).map(str::to_string)
}

/// The specific thing this call acted on, if the arguments name one.
///
/// Preferred over the generic noun, and this is most of what makes a row
/// worth reading: "Wrote Scorecard!B2:B12" says something, "Wrote cells"
/// does not.
fn target(name: &str, args: &str) -> Option<String> {
    let (name, verb) = normalise(name, args);
    let named = |k: &str| field(args, k).filter(|v| !v.trim().is_empty());
    // A call with no selector still happened somewhere: fall back to the
    // handle's unit, so a row reads "Wrote Sheet1" rather than "Wrote cells".
    let where_ = || field(args, "handle").and_then(|h| unit(&h));
    match name {
        "read" | "write" | "format" => named("selector").or_else(where_),
        "export" => named("path").or_else(|| field(args, "handle").and_then(|h| file(&h))),
        "undo" => field(args, "handle").and_then(|h| file(&h)),
        "shell" => named("program"),
        "manual" => named("topic"),
        "plan" => None,
        "struct" => match verb.as_str() {
            "addSheet" | "table" | "name" | "slicer" => named("name"),
            "pivot" | "chart" => named("at").or_else(|| named("title")),
            "insertParagraph" | "createSlide" => named("title").or_else(|| named("text")),
            "macro" => named("name").or_else(|| named("title")),
            "transfer" => named("from"),
            _ => named("selector").or_else(where_),
        },
        _ => None,
    }
    .map(|t| ellipsis(&t, 60))
}

/// Cut a target that is really a paragraph of prose down to a label.
fn ellipsis(s: &str, max: usize) -> String {
    let s = s.split(['\n', '\r']).next().unwrap_or(s).trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// A preposition that reads correctly in front of this call's target.
fn preposition(name: &str, args: &str) -> &'static str {
    let (name, verb) = normalise(name, args);
    match name {
        "read" => "",
        "write" | "format" => "",
        "export" => "to",
        "undo" => "on",
        "shell" => "",
        "manual" => "for",
        "struct" => match verb.as_str() {
            "pivot" | "chart" | "picture" | "insertTable" => "on",
            "transfer" => "from",
            _ => "",
        },
        _ => "",
    }
}

/// The one line a human reads for this call.
///
/// `sentence("struct", {"verb":"chart","at":"Dashboard!A1:H16"}, Done)`
/// is `"Drew a chart on Dashboard!A1:H16"`.
pub fn sentence(name: &str, args: &str, status: Status) -> String {
    let forms = forms_for(name, args);
    let verb = conjugate(forms, status);
    let (_, _, _, generic) = forms;
    match target(name, args) {
        Some(t) => {
            // A specific target replaces the generic noun for the tools whose
            // noun *is* the target ("Wrote cells" -> "Wrote Scorecard!B2"),
            // and is appended for the ones where the noun still carries
            // meaning ("Drew a chart" -> "Drew a chart on Dashboard!A1").
            if keeps_noun(name, args) {
                let prep = preposition(name, args);
                if prep.is_empty() {
                    format!("{verb} {generic} {t}")
                } else {
                    format!("{verb} {generic} {prep} {t}")
                }
            } else {
                let prep = preposition(name, args);
                if prep.is_empty() {
                    format!("{verb} {t}")
                } else {
                    format!("{verb} {prep} {t}")
                }
            }
        }
        None => format!("{verb} {generic}"),
    }
}

/// Whether the generic noun survives alongside a specific target.
fn keeps_noun(name: &str, args: &str) -> bool {
    let (name, verb) = normalise(name, args);
    if name != "struct" {
        return false;
    }
    !matches!(verb.as_str(), "addSheet" | "table" | "name" | "slicer")
}

fn forms_for(name: &str, args: &str) -> Forms {
    let (tool, verb) = normalise(name, args);
    if tool == "struct" {
        if verb == "macro" {
            let action = field(args, "action").unwrap_or_else(|| "write".into());
            if let Some(f) = lookup(MACRO_FORMS, &action) {
                return f;
            }
        }
        if let Some(f) = lookup(STRUCT_FORMS, &verb) {
            return f;
        }
        return ("Change", "Changing", "Changed", "the document");
    }
    lookup(TOOL_FORMS, tool).unwrap_or(("Use", "Using", "Used", "a tool"))
}

// --- grouping ------------------------------------------------------------

/// The bucket a call falls into when many are collapsed into one sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    Read,
    Write,
    Format,
    Structure,
    Export,
    Undo,
    Shell,
    Service,
    Other,
}

/// Which bucket a call belongs to.
pub fn group(name: &str) -> Group {
    match name {
        "read" => Group::Read,
        "write" => Group::Write,
        "format" => Group::Format,
        "export" => Group::Export,
        "undo" => Group::Undo,
        "shell" => Group::Shell,
        "manual" | "plan" => Group::Service,
        "struct" => Group::Structure,
        other if STRUCT_VERBS.contains(&other) => Group::Structure,
        _ => Group::Other,
    }
}

/// How a bucket is counted, and how it is phrased at that count.
///
/// Writes, formats and structural changes count **distinct handles**, not
/// calls. Twelve writes into one sheet is one document changed, which is
/// what happened as far as the human is concerned. t3code counts distinct
/// files for the same reason; everything else counts calls.
fn counts_handles(g: Group) -> bool {
    matches!(g, Group::Write | Group::Format | Group::Structure)
}

fn phrase(g: Group, n: usize, now: bool) -> String {
    let plural = |one: &str, many: &str| if n == 1 { one.to_string() } else { many.to_string() };
    fn verb<'a>(now: bool, past: &'a str, present: &'a str) -> &'a str {
        if now { present } else { past }
    }
    let verb = |past, present| verb(now, past, present);
    match g {
        Group::Read => format!("{} {n} {}", verb("Read", "Reading"), plural("range", "ranges")),
        Group::Write => format!("{} {n} {}", verb("Wrote into", "Writing into"), plural("document", "documents")),
        Group::Format => format!("{} {n} {}", verb("Restyled", "Restyling"), plural("document", "documents")),
        Group::Structure => {
            format!("{} {n} {}", verb("Restructured", "Restructuring"), plural("document", "documents"))
        }
        Group::Export => format!("{} {n} {}", verb("Exported", "Exporting"), plural("file", "files")),
        Group::Undo => format!("{} {n} {}", verb("Undid", "Undoing"), plural("change", "changes")),
        Group::Shell => format!("{} {n} {}", verb("Ran", "Running"), plural("program", "programs")),
        Group::Service => {
            format!("{} the plan and manual {n} {}", verb("Checked", "Checking"), plural("time", "times"))
        }
        Group::Other => format!("{} {n} {}", verb("Used", "Using"), plural("tool", "tools")),
    }
}

/// Collapse a run of calls into one sentence a human can scan.
///
/// `Read 4 ranges, wrote into 2 documents, and ran 1 program.`
///
/// Order is the order the buckets were first seen, so the sentence follows
/// the work rather than an arbitrary enum order. The first clause keeps its
/// capital and the rest are lowercased, which is what makes it read as one
/// sentence instead of a list of headlines.
pub fn summarise(calls: &[(String, String)]) -> String {
    group_sentence(calls, false)
}

/// The same sentence in the present tense, for a batch that is about to
/// run: `Reading 8 ranges and writing into 1 document.`
pub fn summarise_now(calls: &[(String, String)]) -> String {
    group_sentence(calls, true)
}

fn group_sentence(calls: &[(String, String)], now: bool) -> String {
    let mut order: Vec<Group> = Vec::new();
    let mut members: Vec<Vec<(String, String)>> = Vec::new();
    for (name, args) in calls {
        let g = group(name);
        match order.iter().position(|x| *x == g) {
            Some(i) => members[i].push((name.clone(), args.clone())),
            None => {
                order.push(g);
                members.push(vec![(name.clone(), args.clone())]);
            }
        }
    }

    let clauses: Vec<String> = order
        .iter()
        .zip(members.iter())
        .map(|(g, items)| {
            let n = if counts_handles(*g) {
                let mut seen: Vec<String> = Vec::new();
                for (_, args) in items {
                    let key = field(args, "handle").unwrap_or_default();
                    if !seen.contains(&key) {
                        seen.push(key);
                    }
                }
                seen.len()
            } else {
                items.len()
            };
            phrase(*g, n.max(1), now)
        })
        .collect();

    join_clauses(&clauses)
}

/// English list joining, with the serial comma, lowercasing every clause
/// after the first.
fn join_clauses(clauses: &[String]) -> String {
    let parts: Vec<String> =
        clauses.iter().enumerate().map(|(i, c)| if i == 0 { c.clone() } else { lower_first(c) }).collect();
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        2 => format!("{} and {}", parts[0], parts[1]),
        _ => {
            let head = parts[..parts.len() - 1].join(", ");
            format!("{head}, and {}", parts[parts.len() - 1])
        }
    }
}

// --- severity ------------------------------------------------------------

/// How badly a step went, for a reader deciding whether to look.
///
/// The distinction t3code draws and we got wrong once already: a tool that
/// refused is not a broken run. Our capability grader scored a refused call
/// as the model giving up, and it took five instrument fixes to separate
/// "the application said no" from "the harness broke". Encoding it in the
/// type means a renderer cannot make the same mistake by eye.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Nothing is wrong.
    Ok,
    /// The call did not go through, and the model can try again.
    Recoverable,
    /// The run is over: a kill switch, a dead pipe, a spent budget, a
    /// provider that stopped answering.
    Broken,
}

/// Classify one step.
pub fn severity(step: &crate::agent::Step) -> Severity {
    use crate::agent::Step;
    match step {
        Step::Ran { .. } | Step::Answered(_) | Step::NeedsApproval(_) => Severity::Ok,
        Step::Refused(_) => Severity::Recoverable,
        Step::Stopped(_) => Severity::Broken,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_struct_verb_has_words() {
        for verb in STRUCT_VERBS {
            assert!(
                lookup(STRUCT_FORMS, verb).is_some(),
                "struct verb {verb:?} has no entry in STRUCT_FORMS: it would reach a human as raw JSON"
            );
        }
    }

    #[test]
    fn every_tool_on_the_surface_has_words() {
        for t in crate::tools::TOOLS {
            if t.name == "struct" {
                continue; // covered verb by verb above
            }
            assert!(lookup(TOOL_FORMS, t.name).is_some(), "tool {:?} has no entry in TOOL_FORMS", t.name);
        }
        for s in crate::looptools::SERVICES {
            assert!(lookup(TOOL_FORMS, s.name).is_some(), "service {:?} has no entry in TOOL_FORMS", s.name);
        }
    }

    #[test]
    fn the_status_picks_the_tense() {
        let args = r#"{"handle":"excel:plan.xlsx:Sheet1","selector":"Sheet1!A1:C5"}"#;
        assert_eq!(sentence("read", args, Status::Running), "Reading Sheet1!A1:C5");
        assert_eq!(sentence("read", args, Status::Done), "Read Sheet1!A1:C5");
        assert_eq!(sentence("read", args, Status::Failed), "Failed to read Sheet1!A1:C5");
        assert_eq!(sentence("read", args, Status::Refused), "Refused to read Sheet1!A1:C5");
        assert_eq!(sentence("read", args, Status::Stopped), "Stopped reading Sheet1!A1:C5");
    }

    #[test]
    fn a_tool_name_never_reaches_the_sentence() {
        let cases = [
            ("struct", r#"{"handle":"excel:p.xlsx:S1","verb":"chart","at":"Dashboard!A1:H16"}"#),
            ("struct", r#"{"handle":"excel:p.xlsx:S1","verb":"addSheet","name":"Scorecard"}"#),
            ("write", r#"{"handle":"excel:p.xlsx:S1","selector":"B2:B12","values":"1"}"#),
            ("shell", r#"{"program":"hostname","why":"label the report"}"#),
            ("plan", r#"{"steps":"a|b"}"#),
        ];
        for (name, args) in cases {
            let s = sentence(name, args, Status::Done);
            assert!(!s.contains(name) || name == "plan", "{name}: raw tool name leaked into {s:?}");
            assert!(!s.contains('{') && !s.contains('"'), "{name}: raw JSON leaked into {s:?}");
        }
    }

    #[test]
    fn a_specific_target_beats_the_generic_noun() {
        let with = r#"{"handle":"excel:p.xlsx:S1","verb":"addSheet","name":"Scorecard"}"#;
        let without = r#"{"handle":"excel:p.xlsx:S1","verb":"addSheet"}"#;
        assert_eq!(sentence("struct", with, Status::Done), "Added Scorecard");
        assert_eq!(sentence("struct", without, Status::Done), "Added a worksheet");
    }

    #[test]
    fn the_noun_survives_where_it_still_carries_meaning() {
        let args = r#"{"handle":"excel:p.xlsx:S1","verb":"chart","at":"Dashboard!A1:H16"}"#;
        assert_eq!(sentence("struct", args, Status::Done), "Drew a chart on Dashboard!A1:H16");
    }

    #[test]
    fn a_macro_says_which_half_of_the_job_it_did() {
        let h = r#""handle":"excel:p.xlsx:S1","verb":"macro""#;
        let write = format!("{{{h},\"action\":\"write\",\"name\":\"Report\"}}");
        let run = format!("{{{h},\"action\":\"run\",\"title\":\"BuildReport\"}}");
        assert_eq!(sentence("struct", &write, Status::Done), "Wrote a macro Report");
        assert_eq!(sentence("struct", &run, Status::Running), "Running a macro BuildReport");
    }

    #[test]
    fn a_struct_verb_sent_as_a_tool_name_still_reads() {
        // The surface accepts these and `to_action` folds them back into
        // `struct`; the label has to follow or the row goes generic.
        let args = r#"{"handle":"excel:p.xlsx:S1","at":"Dash!A1"}"#;
        assert_eq!(sentence("chart", args, Status::Done), "Drew a chart on Dash!A1");
    }

    #[test]
    fn prose_targets_are_cut_to_a_label() {
        let long = "x".repeat(200);
        let args = format!(r#"{{"handle":"word:r.docx:body","verb":"insertParagraph","text":"{long}"}}"#);
        let s = sentence("struct", &args, Status::Done);
        assert!(s.chars().count() < 90, "a paragraph became the label: {s:?}");
        assert!(s.ends_with('…'), "a cut label should say it was cut: {s:?}");
    }

    #[test]
    fn many_calls_collapse_into_one_sentence() {
        let calls: Vec<(String, String)> = vec![
            ("read".into(), r#"{"handle":"excel:p.xlsx:S1","selector":"A1"}"#.into()),
            ("read".into(), r#"{"handle":"excel:p.xlsx:S1","selector":"A2"}"#.into()),
            ("write".into(), r#"{"handle":"excel:p.xlsx:S1","selector":"B1","values":"1"}"#.into()),
            ("shell".into(), r#"{"program":"hostname","why":"x"}"#.into()),
        ];
        assert_eq!(summarise(&calls), "Read 2 ranges, wrote into 1 document, and ran 1 program");
    }

    #[test]
    fn writes_count_documents_not_calls() {
        // Twelve writes into one sheet is one document changed. Counting the
        // calls would report twelve, which is true of the wire and false of
        // what happened.
        let calls: Vec<(String, String)> = (0..12)
            .map(|i| {
                ("write".to_string(), format!(r#"{{"handle":"excel:p.xlsx:S1","selector":"B{i}","values":"1"}}"#))
            })
            .collect();
        assert_eq!(summarise(&calls), "Wrote into 1 document");
    }

    #[test]
    fn two_documents_are_two() {
        let calls: Vec<(String, String)> = vec![
            ("write".into(), r#"{"handle":"excel:p.xlsx:S1","selector":"B1","values":"1"}"#.into()),
            ("write".into(), r#"{"handle":"word:r.docx:body","selector":"body","values":"x"}"#.into()),
        ];
        assert_eq!(summarise(&calls), "Wrote into 2 documents");
    }

    #[test]
    fn the_sentence_follows_the_work_not_the_enum() {
        let calls: Vec<(String, String)> = vec![
            ("shell".into(), r#"{"program":"hostname","why":"x"}"#.into()),
            ("read".into(), r#"{"handle":"excel:p.xlsx:S1","selector":"A1"}"#.into()),
        ];
        assert_eq!(summarise(&calls), "Ran 1 program and read 1 range");
    }

    #[test]
    fn nothing_summarises_to_nothing() {
        assert_eq!(summarise(&[]), "");
    }

    #[test]
    fn a_refusal_is_not_a_broken_run() {
        use crate::agent::Step;
        assert_eq!(severity(&Step::Refused("bad selector".into())), Severity::Recoverable);
        assert_eq!(severity(&Step::Stopped("kill switch".into())), Severity::Broken);
        assert_eq!(severity(&Step::Ran { tool: "read".into(), detail: String::new() }), Severity::Ok);
    }

    #[test]
    fn the_app_is_available_for_an_icon() {
        assert_eq!(app(r#"{"handle":"excel:plan.xlsx:Sheet1"}"#).as_deref(), Some("excel"));
        assert_eq!(app(r#"{"program":"hostname"}"#), None);
    }

    #[test]
    fn a_handle_splits_into_file_and_unit() {
        assert_eq!(unit("excel:plan.xlsx:Sheet1").as_deref(), Some("Sheet1"));
        assert_eq!(file("excel:plan.xlsx:Sheet1").as_deref(), Some("plan.xlsx"));
        assert_eq!(unit("excel:plan.xlsx").as_deref(), None);
    }
}
