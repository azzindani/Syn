//! Planning a turn that asked for several tools at once.
//!
//! A model sampling a large sheet does not ask for one read. It asks for
//! eight, in one assistant message, and every one of them owes a `tool`
//! reply carrying its own `tool_call_id`. Getting that wrong does not
//! produce a wrong answer, it produces a 400 from the provider on the
//! *next* turn and a run that dies for reasons nobody can see in the
//! transcript.
//!
//! opencode's invariant, from `session/processor.ts`: every announced call
//! id gets **exactly one** terminal event. `ensureToolCall` returns the
//! existing part when an id is seen twice rather than making a second one,
//! and `cleanup()` settles anything still open as an error so no
//! `tool_use` is left unanswered. This module is that invariant, made
//! explicit and decided before anything runs.
//!
//! It adds one thing opencode has no reason to want. opencode's tools are
//! local functions and running the same one twice costs microseconds. Ours
//! is a round trip over a named pipe to a single-threaded COM apartment
//! holding a document a human is looking at, so two identical reads in one
//! batch are worth collapsing into one. Only where that is provably
//! invisible — see [`is_pure`].

use crate::tools::{field, ToolCall};

/// One thing to actually run, and every call id its result answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// The call to dispatch.
    pub call: ToolCall,
    /// Further ids that asked for exactly the same thing and will be given
    /// this slot's result verbatim. Empty in the ordinary case.
    pub also: Vec<String>,
}

impl Slot {
    /// Every id this slot owes an answer to, primary first.
    pub fn ids(&self) -> Vec<String> {
        let mut v = vec![self.call.id.clone()];
        v.extend(self.also.iter().cloned());
        v
    }
}

/// What a turn's calls will do, worked out before any of them run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// In the order the model asked for them.
    pub slots: Vec<Slot>,
    /// Ids that must not be answered at all, with why. Only ever a
    /// repeated id: it has already been answered once, and answering it
    /// twice is what makes the transcript malformed.
    pub dropped: Vec<(String, String)>,
}

impl Plan {
    /// How many round trips this turn will actually cost.
    pub fn runs(&self) -> usize {
        self.slots.len()
    }

    /// How many calls the model asked for.
    pub fn asked(&self) -> usize {
        self.slots.iter().map(|s| 1 + s.also.len()).sum::<usize>() + self.dropped.len()
    }

    /// How many round trips were saved by collapsing identical read-only
    /// calls. Worth reporting: it is the difference between a batch that
    /// fits the budget and one that does not.
    pub fn coalesced(&self) -> usize {
        self.slots.iter().map(|s| s.also.len()).sum()
    }
}

/// Whether running this call twice is indistinguishable from running it
/// once, from the document's point of view and the human's.
///
/// Deliberately a short allow-list rather than a guess. `read` and the
/// manual touch nothing. `export` is pure only in the two shapes that
/// return a description instead of writing a file — `summary` and
/// `preview` — and the moment it is given a `path` it is a file write and
/// is not on this list.
///
/// Everything else is excluded even where it looks idempotent. Writing the
/// same values to the same cells twice really is harmless; `addSheet` with
/// the same name twice really is not, and the second failing is
/// information the model should get rather than have hidden. When in
/// doubt, run it: the cost of running a duplicate is one pipe round trip,
/// and the cost of wrongly collapsing one is a document that does not say
/// what anyone thinks it says.
pub fn is_pure(name: &str, args: &str) -> bool {
    match name {
        "read" | "manual" => true,
        "export" => {
            field(args, "path").is_none()
                && matches!(field(args, "format").as_deref(), Some("summary") | Some("preview"))
        }
        _ => false,
    }
}

/// Two calls are the same request when the tool and the arguments match.
///
/// Byte comparison of the raw argument text, not a parsed comparison: a
/// difference in key order is a different request as far as this is
/// concerned, and refusing to collapse it costs one round trip, whereas
/// collapsing two calls that differ somewhere this does not understand
/// costs a wrong document.
fn same_request(a: &ToolCall, b: &ToolCall) -> bool {
    a.name == b.name && a.arguments == b.arguments
}

/// Decide what a turn's calls will do.
pub fn plan(calls: &[ToolCall]) -> Plan {
    let mut out = Plan::default();
    for call in calls {
        // A repeated id is the provider having duplicated a call, not the
        // model having asked twice. It is already going to be answered.
        // Answering it a second time puts two `tool` messages with one
        // `tool_call_id` into the transcript, which the stricter providers
        // reject outright and the looser ones silently mis-pair.
        let seen = out.slots.iter().any(|s| s.ids().contains(&call.id))
            || out.dropped.iter().any(|(i, _)| *i == call.id);
        if seen {
            out.dropped.push((call.id.clone(), format!("duplicate call id {:?} in one turn", call.id)));
            continue;
        }

        if is_pure(&call.name, &call.arguments)
            && let Some(slot) = out.slots.iter_mut().find(|s| same_request(&s.call, call))
        {
            slot.also.push(call.id.clone());
            continue;
        }

        out.slots.push(Slot { call: call.clone(), also: Vec::new() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall { id: id.into(), name: name.into(), arguments: args.into() }
    }

    const R1: &str = r#"{"handle":"excel:p.xlsx:S1","selector":"Sheet1!A1:B2"}"#;
    const R2: &str = r#"{"handle":"excel:p.xlsx:S1","selector":"Sheet1!C1:D2"}"#;

    #[test]
    fn an_ordinary_batch_is_left_exactly_as_it_came() {
        let calls = vec![tc("a", "read", R1), tc("b", "read", R2), tc("c", "write", R1)];
        let p = plan(&calls);
        assert_eq!(p.runs(), 3);
        assert_eq!(p.coalesced(), 0);
        assert!(p.dropped.is_empty());
        let order: Vec<&str> = p.slots.iter().map(|s| s.call.id.as_str()).collect();
        assert_eq!(order, ["a", "b", "c"], "the model's order must survive");
    }

    #[test]
    fn identical_reads_cost_one_round_trip_and_answer_every_id() {
        let calls = vec![tc("a", "read", R1), tc("b", "read", R2), tc("c", "read", R1)];
        let p = plan(&calls);
        assert_eq!(p.runs(), 2, "the repeated read should run once");
        assert_eq!(p.coalesced(), 1);
        assert_eq!(p.slots[0].ids(), vec!["a".to_string(), "c".to_string()]);
        assert_eq!(p.asked(), 3, "all three ids are still owed an answer");
    }

    #[test]
    fn a_repeated_write_is_run_twice() {
        // Collapsing it would hide the second one's result, and for a
        // structural verb the second result is usually the interesting one
        // ("a sheet named Scorecard already exists").
        let add = r#"{"handle":"excel:p.xlsx:S1","verb":"addSheet","name":"Scorecard"}"#;
        let p = plan(&[tc("a", "struct", add), tc("b", "struct", add)]);
        assert_eq!(p.runs(), 2);
        assert_eq!(p.coalesced(), 0);
    }

    #[test]
    fn a_duplicated_call_id_is_answered_once_and_only_once() {
        // The provider defect, not the model's doing. Two `tool` messages
        // sharing a `tool_call_id` is a malformed transcript.
        let p = plan(&[tc("a", "read", R1), tc("a", "write", R2)]);
        assert_eq!(p.runs(), 1);
        assert_eq!(p.dropped.len(), 1);
        assert_eq!(p.dropped[0].0, "a");
        assert!(p.dropped[0].1.contains("duplicate call id"));
    }

    #[test]
    fn every_announced_id_is_accounted_for_exactly_once() {
        // The invariant the whole module exists for. Whatever the batch,
        // the set of ids that get answered plus the set dropped is the set
        // announced, with no id in both and none missing.
        let calls = vec![
            tc("a", "read", R1),
            tc("b", "read", R1),
            tc("c", "write", R2),
            tc("a", "read", R2),
            tc("d", "manual", r#"{"topic":"excel"}"#),
            tc("e", "manual", r#"{"topic":"excel"}"#),
        ];
        let p = plan(&calls);
        let mut answered: Vec<String> = p.slots.iter().flat_map(|s| s.ids()).collect();
        answered.extend(p.dropped.iter().map(|(i, _)| i.clone()));
        answered.sort();

        let mut announced: Vec<String> = calls.iter().map(|c| c.id.clone()).collect();
        announced.sort();

        assert_eq!(answered, announced, "every id announced must be answered or dropped, once");
    }

    #[test]
    fn export_is_pure_only_when_it_writes_nothing() {
        assert!(is_pure("export", r#"{"handle":"h","format":"summary"}"#));
        assert!(is_pure("export", r#"{"handle":"h","format":"preview"}"#));
        assert!(!is_pure("export", r#"{"handle":"h","format":"xlsx","path":"C:/out.xlsx"}"#));
        // A path makes it a file write whatever the format says.
        assert!(!is_pure("export", r#"{"handle":"h","format":"summary","path":"C:/out.txt"}"#));
    }

    #[test]
    fn nothing_that_reaches_a_document_is_treated_as_pure() {
        for name in ["write", "format", "struct", "undo", "shell", "plan", "chart", "addSheet"] {
            assert!(!is_pure(name, "{}"), "{name} must not be collapsed");
        }
    }

    #[test]
    fn arguments_that_differ_at_all_are_different_requests() {
        let a = r#"{"handle":"h","selector":"A1"}"#;
        let b = r#"{"selector":"A1","handle":"h"}"#; // same meaning, different text
        let p = plan(&[tc("1", "read", a), tc("2", "read", b)]);
        assert_eq!(p.runs(), 2, "key order is not worth the risk of collapsing");
    }

    #[test]
    fn an_empty_turn_plans_nothing() {
        let p = plan(&[]);
        assert_eq!(p.runs(), 0);
        assert_eq!(p.asked(), 0);
    }
}
