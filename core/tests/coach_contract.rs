//! The error coaching keys on words the helpers send. If a helper's
//! message is reworded, its advice stops matching and nobody notices: the
//! model just gets the bare error again. So every rule that tracks our own
//! wording is checked against the source that says it.

use std::path::Path;

#[test]
fn every_coached_error_is_still_worded_that_way_at_its_source() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut missing = Vec::new();
    for r in core::coach::RULES {
        if r.from == "com" {
            continue;
        }
        let src = std::fs::read_to_string(root.join(r.from)).unwrap_or_else(|e| panic!("{}: {e}", r.from));
        if !src.contains(r.needle) {
            missing.push(format!("{:?} is no longer in {}", r.needle, r.from));
        }
    }
    assert!(missing.is_empty(), "coaching that can no longer match:\n{}", missing.join("\n"));
}

#[test]
fn a_windows_code_is_written_the_way_office_host_prints_it() {
    // office-host prints `com 0x{HResult:X8}`: eight upper-case hex digits.
    for r in core::coach::RULES.iter().filter(|r| r.from == "com" && r.needle.starts_with("0x")) {
        let hex = &r.needle[2..];
        assert_eq!(hex.len(), 8, "{}", r.needle);
        assert!(hex.chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()), "{}", r.needle);
    }
}
