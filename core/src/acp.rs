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
pub fn sanitise_formula(s: &str) -> String {
    let mut t = s.trim_start_matches(['=', '+', '-', '@', '\t', '\r', '\n', '\0']).replace('\0', "");
    // Strip any remaining leading control whitespace.
    t = t.trim_start().to_string();
    if t.len() > 1024 {
        t.truncate(1024);
    }
    t
}

#[cfg(test)]
mod tests {
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
