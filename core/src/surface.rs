//! The one place the two layers meet.
//!
//! `tools` holds what reaches the world; `looptools` holds what the loop
//! answers itself. They are separate everywhere except here, because the
//! tool-call API is the only way a model can invoke anything, so it has to
//! be handed one flat list.
//!
//! Everything else stays apart: the world tools keep their own fingerprint
//! (the rug-pull pin, which must not move when a manual page is edited),
//! their own gates, and their own outward publication through `mcpgate`.

use crate::{looptools, tools};

/// Every tool offered on this run, world tools first.
///
/// Order is stable — world tools in their declared order, then whichever
/// loop services are switched on — so two runs made the same way produce
/// the same list.
pub fn tools_json() -> String {
    let world = tools::tools_json();
    let services = looptools::visible();
    if services.is_empty() {
        return world;
    }
    let mut s = world.trim_end().trim_end_matches(']').to_string();
    for t in services {
        s.push(',');
        s.push_str(&tools::spec_json(t));
    }
    s.push(']');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_carries_both_layers_and_stays_valid_json() {
        let j = tools_json();
        for name in ["read", "write", "struct", "shell"] {
            assert!(j.contains(&format!("\"name\":\"{name}\"")), "{name} missing from the surface");
        }
        assert!(j.starts_with('[') && j.ends_with(']'));
        assert_eq!(j.matches("\"type\":\"function\"").count(), tools::TOOLS.len() + looptools::visible().len());
    }

    #[test]
    fn editing_a_manual_page_cannot_move_the_security_pin() {
        // The reason for the split. `surface_fingerprint` is pinned at
        // approval so a tool that changes underneath you can be
        // quarantined as a rug-pull. It covers the tools that reach a
        // document, and nothing else: while the manual sat in that array,
        // fixing a typo in a sentence of documentation raised the alarm.
        let before = tools::surface_fingerprint();
        let pinned_over: Vec<&str> = tools::TOOLS.iter().map(|t| t.name).collect();
        assert!(!pinned_over.contains(&"manual"), "the pin must not cover the manual");
        assert!(!pinned_over.contains(&"plan"), "the pin must not cover the plan");
        assert_eq!(before, tools::surface_fingerprint());
    }
}
