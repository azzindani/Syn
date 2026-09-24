//! The Rust side and the C# sidecars must agree on the wire, and this
//! checks it without Office, without dotnet, and without a live document.
//!
//! Why it exists: every verb added to this project so far has been added in
//! two places, and the gap between them is only discovered by driving real
//! Office — which needs a Windows machine with Microsoft 365 on it, and is
//! therefore the one test that cannot be run from a cloud session or a CI
//! runner. That makes adding a tool remotely a guess.
//!
//! The wire is a string. `hand::envelope_for` turns a `Call` into a method
//! name, and each sidecar dispatches on that name in a `method switch`.
//! Both sides are text in this repository, so the agreement between them is
//! checkable by reading them. A verb added to the Rust side with no handler
//! behind it now fails here, in under a second, on any machine.
//!
//! What this does NOT check: that the handler is correct, that COM accepts
//! the arguments, or that the document ends up right. Those need Office and
//! always will. This only catches the case that has actually bitten —
//! sending a method nobody implements.

use std::fs;
use std::path::PathBuf;

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

/// Read a source file with its line endings normalised.
///
/// `core.autocrlf` is true in this repository, so a Windows checkout gets
/// CRLF and a Linux one gets LF for the very same commit. The first
/// version of this test looked for `"\n}\n"` and therefore passed on two
/// operating systems and failed on the third — caught by CI, which is the
/// only place the difference exists. Reading every source through here
/// means no check below can depend on which machine cloned the repo.
fn source(rel: &str) -> String {
    fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}")).replace('\r', "")
}

/// Every quoted identifier inside `envelope_for`, which is every method the
/// Rust side can put on the wire.
fn methods_rust_can_send() -> Vec<String> {
    let src = source("core/src/hand.rs");
    let from = src.find("pub fn envelope_for").expect("envelope_for");
    let body = &src[from..];
    let to = body.find("\n}\n").expect("end of envelope_for");
    let mut found = quoted_words(&body[..to]);
    // Methods sent from outside `envelope_for` too -- `open` is one, and
    // it lives apart because it names no open document. A method added in
    // some new helper must be covered as well, or this test quietly stops
    // guarding the thing it was written for.
    let mut rest = src.as_str();
    while let Some(i) = rest.find("envelope(\"") {
        rest = &rest[i + 10..];
        if let Some(j) = rest.find(char::from(34))
            && is_word(&rest[..j])
        {
            found.push(rest[..j].to_string());
        }
    }
    // The table-driven verbs are sent under their own names by one generic
    // arm, so no literal names them in `envelope_for`: the table does.
    found.extend(core::tools::OFFICE_VERBS.iter().map(|v| v.name.to_string()));
    found.sort();
    found.dedup();
    found
}

/// Every method name a sidecar dispatches on: `"name" when ...` or
/// `"name" => ...` inside its `method switch`.
fn methods_a_sidecar_handles(path: &str) -> Vec<String> {
    let src = source(path);
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        if !t.starts_with('"') {
            continue;
        }
        let Some(end) = t[1..].find('"') else { continue };
        let name = &t[1..=end];
        let rest = t[end + 2..].trim_start();
        if (rest.starts_with("when") || rest.starts_with("=>")) && is_word(name) {
            out.push(name.to_string());
        }
    }
    // The other dispatch shape: `if (method == "open")`, used by the one
    // request that has to be answered before the per-application switch
    // because it names no open document.
    let mut rest = src.as_str();
    while let Some(i) = rest.find("method == \"") {
        rest = &rest[i + "method == \"".len()..];
        if let Some(j) = rest.find(char::from(34))
            && is_word(&rest[..j])
        {
            out.push(rest[..j].to_string());
        }
    }
    out
}

fn quoted_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find('"') {
        rest = &rest[i + 1..];
        let Some(j) = rest.find('"') else { break };
        let w = &rest[..j];
        if is_word(w) {
            out.push(w.to_string());
        }
        rest = &rest[j + 1..];
    }
    out.sort();
    out.dedup();
    out
}

fn is_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphabetic())
}

/// Methods the Rust side sends that no sidecar implements, on purpose.
///
/// `undo` is the only one. The runbook says why: undo for a live handle
/// belongs to the sidecar's `.bak` and to the application's own undo stack,
/// and taking a relay snapshot would make `undo` look available when it is
/// not. The op is sent, refused by the sidecar, and reported — which is the
/// intended behaviour, not a missing handler. If that ever changes, delete
/// the entry and the test starts requiring a handler.
const DELIBERATELY_UNIMPLEMENTED: &[&str] = &["undo"];

#[test]
fn every_method_the_rust_side_sends_has_a_handler_behind_it() {
    let rust = methods_rust_can_send();
    assert!(rust.len() > 10, "envelope_for parsed as {rust:?}, which cannot be right");

    let mut handled = methods_a_sidecar_handles("sidecar-csharp/Host/Program.cs");
    handled.extend(methods_a_sidecar_handles("sidecar-csharp/Uia/Program.cs"));
    handled.extend(DELIBERATELY_UNIMPLEMENTED.iter().map(|s| s.to_string()));

    let orphans: Vec<&String> = rust.iter().filter(|m| !handled.contains(m)).collect();
    assert!(
        orphans.is_empty(),
        "these methods go on the wire and no sidecar dispatches on them: {orphans:?}\n\
         Add a case to the `method switch` in sidecar-csharp/Host/Program.cs (or Uia), \
         or list it in DELIBERATELY_UNIMPLEMENTED with the reason."
    );
}

#[test]
fn the_struct_verbs_the_model_can_call_all_reach_the_wire() {
    // The other half of the same gap, one layer up: a verb offered in the
    // `struct` schema that `envelope_for` has no case for is refused only
    // once a live hand is attached, which is to say on someone's desk
    // rather than in CI.
    let wire = methods_rust_can_send();
    // These are handled by the in-memory model or by the relay and never
    // become an office-rpc method of their own.
    let not_on_the_wire = ["transfer"];
    let missing: Vec<&&str> = core::tools::STRUCT_VERBS
        .iter()
        .filter(|v| !wire.contains(&v.to_string()) && !not_on_the_wire.contains(*v))
        .collect();
    assert!(
        missing.is_empty(),
        "these struct verbs are offered to the model but never reach a hand: {missing:?}"
    );
}

#[test]
fn the_sidecars_are_where_the_test_thinks_they_are() {
    // A rename that silently emptied the lists above would make both tests
    // pass by checking nothing.
    for p in ["sidecar-csharp/Host/Program.cs", "sidecar-csharp/Uia/Program.cs"] {
        let n = methods_a_sidecar_handles(p).len();
        assert!(n >= 4, "{p} parsed as only {n} handled methods");
    }
}

/// Every method `sidecar-lo/lo_host.py` handles: the keys of its dispatch
/// tables (`"read": lambda ...`, `"write": word_write`) and the two it
/// tests for by name (`method == "open"`).
fn methods_the_libreoffice_helper_handles() -> Vec<String> {
    let src = source("sidecar-lo/lo_host.py");
    let mut found = Vec::new();
    for (i, _) in src.match_indices("\": ") {
        let after = &src[i + 3..];
        if !(after.starts_with("lambda") || after.starts_with("word_")) {
            continue;
        }
        let before = &src[..i];
        if let Some(q) = before.rfind('"') {
            let name = &before[q + 1..];
            if is_word(name) {
                found.push(name.to_string());
            }
        }
    }
    for (i, _) in src.match_indices("method == \"") {
        let rest = &src[i + 11..];
        if let Some(j) = rest.find('"') {
            found.push(rest[..j].to_string());
        }
    }
    found.sort();
    found.dedup();
    found
}

#[test]
fn the_libreoffice_helper_handles_only_what_rust_sends_and_the_core_of_it() {
    // The Linux helper is a subset of office-host by design (no pivots, no
    // VBA). What it must not do is answer a method nothing sends -- a
    // misspelt verb that is dead on arrival -- or lose one of the core ops
    // the live tests in tests/test_lo_live.py rely on.
    let sent = methods_rust_can_send();
    let handled = methods_the_libreoffice_helper_handles();
    let dead: Vec<&String> = handled.iter().filter(|m| !sent.contains(m)).collect();
    assert!(dead.is_empty(), "sidecar-lo/lo_host.py handles {dead:?}, which core/src/hand.rs never sends");
    for core in ["open", "read", "write", "format", "export", "addSheet", "chart", "insertParagraph", "insertTable", "createSlide"] {
        assert!(handled.iter().any(|m| m == core), "sidecar-lo/lo_host.py no longer handles {core}: {handled:?}");
    }
}
