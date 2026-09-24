//! Tools the loop answers itself.
//!
//! Two kinds of thing were living in one array. `read/write/format/struct/
//! export/undo` and `shell` reach the world: they go through `Runner`, meet
//! the kill switch, the app allowlist, the doom-loop gate and the event
//! feed, they snapshot so `undo` means something, and six of them are the
//! vocabulary `mcpgate` publishes outward. `manual` and `plan` reach
//! nothing. They are answered inside `Agent::dispatch`, touch no document,
//! need no gate and have nothing to undo.
//!
//! Mixing them cost three specific things:
//!
//! 1. `tools::surface_fingerprint` is a security control — it pins the tool
//!    surface at approval so a description or schema that changes
//!    underneath you can be quarantined as a rug-pull. With the manual in
//!    the same array, **fixing a typo in a manual page tripped the tamper
//!    alarm.** An alarm that fires on routine content edits is one people
//!    learn to ignore.
//! 2. `tools::to_action` resolves document ops, and had to begin with two
//!    early returns for things that are not document ops, each jumping the
//!    `handle` check that every real op requires.
//! 3. The closed-schema doctrine — caps, `additionalProperties:false`,
//!    refuse rather than truncate — is reasoning about calls that reach a
//!    live document. Applying it to a struct field is cargo cult.
//!
//! The model still sees one flat list, because the tool-call API is the
//! only way it can invoke anything. That merge happens at the wire, in
//! `surface`, and nowhere else.

use crate::tools::{ToolSpec, field};

/// Whether the manual is offered. `AGENT_MANUAL=0` takes it off.
pub fn manual_enabled() -> bool {
    !matches!(std::env::var("AGENT_MANUAL").as_deref(), Ok("0"))
}

/// Whether the model keeps a plan of its own. `AGENT_PLAN=0` takes off both
/// the tool and the per-turn status note.
pub fn plan_enabled() -> bool {
    !matches!(std::env::var("AGENT_PLAN").as_deref(), Ok("0"))
}

/// Every loop service, whether or not it is switched on.
pub const SERVICES: &[ToolSpec] = &[
    ToolSpec {
        name: "manual",
        description: "Read how a tool behaves before you use it. These tools drive applications a human has open, and each one has behaviour worth knowing first: which write fills a whole range in one call, why a chart anchored to a cell overlaps its neighbour, what a pivot cannot do. It does NOT tell you what to build or in what order -- that is yours to decide, and `plan` is where you record it. Touches no document and does not count as progress. Costs one step and routinely saves twenty. Example: manual{\"topic\":\"excel\"} before the first write into a workbook.",
        params: r#"{"type":"object","properties":{"topic":{"type":"string","enum":["index","loop","excel","word","powerpoint"],"description":"which manual. Start with index if unsure."}},"required":["topic"],"additionalProperties":false}"#,
    },
    ToolSpec {
        name: "plan",
        description: "Your own plan for this job, in your own words. Call it with `steps` before you start: the steps are not checked against anything and there is no expected answer -- the plan exists so that twenty calls later, deep in a transcript, you can see what you decided and what is left. Mark items finished with `done` as you go. Call it again with new `steps` whenever the work turns out differently; the plan is yours to revise, not a contract. It is shown back to you every turn alongside the step budget. Touches no document and does not count as progress. Example: plan{\"steps\":\"survey the data|build the summary sheet|chart it|write the report\"} then later plan{\"done\":\"1,2\"}.",
        params: r#"{"type":"object","properties":{"steps":{"type":"string","maxLength":4000,"description":"the whole plan, steps joined by | -- replaces any previous plan"},"done":{"type":"string","maxLength":200,"description":"1-based step numbers now finished, joined by commas, e.g. 1,2"},"note":{"type":"string","maxLength":500,"description":"optional: anything about the plan worth remembering"}},"required":[],"additionalProperties":false}"#,
    },
];

/// The services switched on for this run.
pub fn visible() -> Vec<&'static ToolSpec> {
    SERVICES
        .iter()
        .filter(|t| match t.name {
            "manual" => manual_enabled(),
            "plan" => plan_enabled(),
            _ => true,
        })
        .collect()
}

pub fn is_service(name: &str) -> bool {
    SERVICES.iter().any(|t| t.name == name)
}

/// What a loop-service call resolves to. No handle: none of these address
/// a document, and demanding one would teach the model that reading the
/// instructions requires something open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Service {
    Manual(String),
    Plan { steps: Option<String>, done: Option<String>, note: Option<String> },
}

/// Resolve a call if it is a loop service, or `None` if it belongs to the
/// world tools. Keeping this a separate entry point is the whole point of
/// the split: `tools::to_action` no longer opens with special cases for
/// calls that never reach a hand.
pub fn resolve(name: &str, args: &str) -> Option<Result<Service, String>> {
    match name {
        "manual" => Some(if manual_enabled() {
            match field(args, "topic") {
                Some(t) => Ok(Service::Manual(t)),
                None => Err("manual: missing required field topic".into()),
            }
        } else {
            Err("unknown tool \"manual\": not on the exposed surface".into())
        }),
        "plan" => Some(if plan_enabled() {
            let s = Service::Plan {
                steps: field(args, "steps"),
                done: field(args, "done"),
                note: field(args, "note"),
            };
            match &s {
                Service::Plan { steps: None, done: None, note: None } => {
                    Err("plan: give steps, done, or both".into())
                }
                _ => Ok(s),
            }
        } else {
            Err("unknown tool \"plan\": not on the exposed surface".into())
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_service_call_never_asks_for_a_handle() {
        match resolve("manual", r#"{"topic":"excel"}"#).expect("a service") {
            Ok(Service::Manual(t)) => assert_eq!(t, "excel"),
            other => panic!("{other:?}"),
        }
        match resolve("plan", r#"{"steps":"a|b"}"#).expect("a service") {
            Ok(Service::Plan { steps, .. }) => assert_eq!(steps.as_deref(), Some("a|b")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_world_tool_is_not_a_service_and_falls_through() {
        // The whole split: `resolve` must not claim a call it cannot
        // answer, or a write would be swallowed here and never reach a
        // hand while the model was told it succeeded.
        assert!(resolve("write", r#"{"handle":"h","selector":"A1","values":"x"}"#).is_none());
        assert!(resolve("shell", r#"{"program":"hostname","why":"x"}"#).is_none());
        assert!(!is_service("write"));
        assert!(is_service("manual") && is_service("plan"));
    }

    #[test]
    fn the_manual_enum_cannot_drift_from_the_pages() {
        // A topic in the schema with no page behind it sends the model to
        // something that does not exist; a page with no topic is one it
        // may never ask for.
        let params = SERVICES.iter().find(|t| t.name == "manual").unwrap().params;
        for t in crate::manual::topics() {
            assert!(params.contains(&format!("\"{t}\"")), "topic {t} is missing from the schema enum");
        }
        let at = params.find("\"enum\":[").unwrap() + "\"enum\":[".len();
        let list = &params[at..params[at..].find(']').unwrap() + at];
        assert_eq!(list.split(',').count(), crate::manual::topics().len());
    }

    #[test]
    fn both_services_are_offered_by_default() {
        let on: Vec<&str> = visible().iter().map(|t| t.name).collect();
        assert_eq!(on, vec!["manual", "plan"]);
    }

    #[test]
    fn an_empty_plan_call_is_refused() {
        let err = resolve("plan", "{}").expect("a service").unwrap_err();
        assert!(err.contains("steps, done, or both"), "{err}");
    }
}
