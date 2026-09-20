//! Artifact Context Packets (mcp-office ACP v1 pattern): 3-level responses so
//! callers request only needed context (compaction-by-construction), plus
//! formula-injection sanitiser applied to every cell write.

/// Response depth. Higher levels nest lower ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Index,
    Focused,
    Deep,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub level: Level,
    pub summary: String,
    pub content: Option<String>,
    pub detail: Option<String>,
    pub annotations: Vec<(String, String)>,
}

pub fn packet(level: Level, summary: &str, content: Option<&str>, detail: Option<&str>) -> Packet {
    Packet {
        level,
        summary: summary.into(),
        content: if level >= Level::Focused { content.map(str::to_string) } else { None },
        detail: if level >= Level::Deep { detail.map(str::to_string) } else { None },
        annotations: Vec::new(),
    }
}

/// Strip Excel formula-injection prefixes + control chars; cap at 1024.
///
/// For text arriving from outside — a web page, a tool result, a file the
/// user did not write — on its way into a document a human will later
/// open. **Not** for the model's own `write`: see [`cell_value`].
pub fn sanitise_formula(s: &str) -> String {
    let mut t = s.trim_start_matches(['=', '+', '-', '@', '\t', '\r', '\n', '\0']).replace('\0', "");
    // Strip any remaining leading control whitespace.
    t = t.trim_start().to_string();
    if t.len() > 1024 {
        t.truncate(1024);
    }
    t
}

/// One cell as the model asked for it, with the characters that are never
/// a cell's contents removed and a length bound applied.
///
/// Deliberately keeps a leading `=`. Writing a formula and reading back its
/// one-cell answer is the central capability claim of this project — it is
/// in the system prompt, in the `write` tool's description and in the
/// manual — and [`sanitise_formula`] was being applied to the model's own
/// writes, which stripped the `=` and stored the formula as text.
///
/// The live path never did this: `hand.rs` puts the text on the wire and
/// real Excel evaluates it. So the two tiers disagreed, and the one a
/// cloud session can run was the one that silently could not do the thing.
/// Every offline test of a formula was passing on a stored string.
///
/// This is not a hole. `sanitise_formula` exists for text arriving from
/// somewhere else, and that is still where it is used. A `write` is the
/// model deliberately authoring a cell, its text already goes verbatim to
/// the application on the live path, and an observation coming back is
/// fenced as untrusted either way.
pub fn cell_value(s: &str) -> String {
    let mut t: String = s.chars().filter(|c| *c == '\t' || !c.is_control()).collect();
    t = t.trim_matches(['\t', ' ']).to_string();
    if t.chars().count() > 1024 {
        t = t.chars().take(1024).collect();
    }
    t
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_formula_the_model_wrote_keeps_its_equals_sign() {
        // The capability this project is built on. The document model used
        // to store "SUM(A1:A3)" while live Excel got "=SUM(A1:A3)", so the
        // offline tier quietly could not do the one thing the prompt, the
        // tool description and the manual all promise.
        assert_eq!(cell_value("=SUM(A1:A3)"), "=SUM(A1:A3)");
        assert_eq!(cell_value("=COUNTIF(A:A,7)"), "=COUNTIF(A:A,7)");
        assert_eq!(cell_value("-5"), "-5");
        assert_eq!(cell_value("+1"), "+1");
    }

    #[test]
    fn a_cell_still_cannot_carry_control_characters_or_run_long() {
        assert_eq!(cell_value("a\u{0}b"), "ab");
        assert_eq!(cell_value("a\rb"), "ab");
        assert_eq!(cell_value("  x  "), "x");
        assert_eq!(cell_value(&"x".repeat(2000)).chars().count(), 1024);
    }

    #[test]
    fn text_from_outside_is_still_defanged() {
        // `sanitise_formula` keeps its job; it just no longer has the
        // model's own writes as a caller.
        assert_eq!(sanitise_formula("=cmd|' /c calc'!A1"), "cmd|' /c calc'!A1");
        assert_eq!(sanitise_formula("@SUM(1)"), "SUM(1)");
    }

    use super::*;

    #[test]
    fn levels_gate_content() {
        let p = packet(Level::Index, "s", Some("c"), Some("d"));
        assert!(p.content.is_none() && p.detail.is_none());
        let p = packet(Level::Focused, "s", Some("c"), Some("d"));
        assert_eq!(p.content.as_deref(), Some("c"));
        assert!(p.detail.is_none());
        let p = packet(Level::Deep, "s", Some("c"), Some("d"));
        assert_eq!(p.detail.as_deref(), Some("d"));
    }

    #[test]
    fn formula_prefixes_stripped() {
        assert_eq!(sanitise_formula("=1+1"), "1+1");
        assert_eq!(sanitise_formula("@SUM(A1)"), "SUM(A1)");
        assert_eq!(sanitise_formula("+cmd"), "cmd");
        assert_eq!(sanitise_formula("plain"), "plain");
        assert_eq!(sanitise_formula("=HYPERLINK(\"http://x\")"), "HYPERLINK(\"http://x\")");
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    #[test]
    fn level_ordering_and_defaults() {
        assert!(Level::Index < Level::Focused && Level::Focused < Level::Deep);
        let p = packet(Level::Deep, "s", None, None);
        assert!(p.content.is_none() && p.detail.is_none());
        assert!(p.annotations.is_empty());
        assert_eq!(p.summary, "s");
    }

    #[test]
    fn sanitise_minus_control_and_cap() {
        assert_eq!(sanitise_formula("-5"), "5");
        assert_eq!(sanitise_formula("\t\n=2+2"), "2+2");
        assert_eq!(sanitise_formula("a\0b"), "ab");
        assert_eq!(sanitise_formula(""), "");
        let long = "a".repeat(2000);
        assert_eq!(sanitise_formula(&long).len(), 1024);
        assert_eq!(sanitise_formula(&"a".repeat(1024)).len(), 1024);
    }
}
