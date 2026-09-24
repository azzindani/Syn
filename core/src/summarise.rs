//! Replacing the old half of a transcript with a summary, keeping the
//! recent half word for word.
//!
//! Pruning (`agent::compact`) shortens old tool *results* and leaves every
//! message in place. It buys room once. When a run is long enough that even
//! the shortened transcript will not fit, the only thing left is to stop
//! carrying the early history verbatim — and the mistake is to drop it,
//! because then the model forgets the goal it was given three hundred steps
//! ago.
//!
//! opencode's answer, ported here: split the transcript at a turn boundary
//! so that the most recent quarter of the budget survives **verbatim**, have
//! the model write a structured summary of everything before it, and send
//! `[summary] + [verbatim tail]`. What the model reads is its own account of
//! the early work followed by the exact text of the recent work.
//!
//! Two details carry most of the value and neither is obvious:
//!
//! * The split point is a **turn boundary**, never an arbitrary message.
//!   Cutting between an assistant's tool calls and their results leaves the
//!   provider with a `tool_calls` block whose answers are missing, which is
//!   a hard 400 from every provider that checks.
//! * If the summarisation request would *itself* overflow, it is not sent.
//!   opencode has the same guard. A compaction that cannot fit is not a
//!   smaller problem than the one it was called to solve.

use crate::provider::Msg;

/// How much recent history survives untouched, as a fraction of the budget.
/// opencode uses 25%.
const PRESERVE_FRACTION: usize = 4;

/// Floor and ceiling on that, in characters. opencode clamps to 2,000 and
/// 15,000 *tokens*; four characters to the token, as everywhere else in
/// this crate.
const PRESERVE_MIN: usize = 8_000;
const PRESERVE_MAX: usize = 60_000;

/// How much of one tool result goes into the text the summariser reads.
/// Matching `agent::PRUNED_RESULT`, and for the same reason: a grid dump
/// is recognisable from its first two thousand characters.
const SERIALISED_RESULT: usize = 2_000;

/// Room left for the summary itself. A summary that cannot be written is
/// worse than no compaction, so this is reserved before anything else.
const SUMMARY_ROOM: usize = 16_000;

/// The shape the summary must take.
///
/// Ported from opencode's `SUMMARY_TEMPLATE`. The headings are not
/// decoration: a free-form summary loses the thing that matters most, which
/// is the exact strings — handles, selectors, sheet names, the error the
/// application gave back. "Preserve exact file paths, symbols, commands,
/// error strings" is the line doing the work.
pub const TEMPLATE: &str = "\
Output exactly the Markdown structure shown inside <template> and keep the \
section order unchanged. Do not include the <template> tags in your response.
<template>
## Objective
- [one or two brief sentences describing what the user asked for]

## Important Details
- [constraints, decisions and why, facts established, or \"(none)\"]

## Work State
### Completed
- [finished work and verified facts, naming the handles and what is in them now; otherwise \"(none)\"]

### Active
- [work in progress or partly done; otherwise \"(none)\"]

### Blocked
- [blockers, refusals, things that failed and why; otherwise \"(none)\"]

## Next Move
1. [the immediate next action, or \"(none)\"]
2. [the one after, if known, or \"(none)\"]

## Open Handles
- [handle: what it is and what state it is in, or \"(none)\"]
</template>

Rules:
- Keep every section, even when empty.
- Terse bullets, not prose paragraphs.
- Preserve exact handles, selectors, sheet and file names, formulas and \
error strings. A number you do not carry over is lost.
- Do not mention this summary or that the history was shortened.";

/// Added when there is already a summary to fold in.
///
/// The second sentence is the important one. A model asked to "update" a
/// summary will quietly drop what the recent conversation did not mention,
/// and what it drops is usually the original goal.
pub const UPDATE: &str = "\
The <prior-summary> covers everything before <conversation>. Write one new \
summary combining both. The prior summary is discarded after this: anything \
you do not carry into the new one is lost.
- Carry forward the objective, constraints and decisions from the prior \
summary even when the conversation does not mention them again.
- The conversation is newer. Where they disagree, the conversation wins: \
state the corrected fact and drop the old claim.
- Move finished work from Active to Completed.";

/// One message, rendered for the summariser to read.
///
/// Tool calls and their results are kept as call-and-answer pairs rather
/// than flattened, because "what did I try and what did it say" is most of
/// what a summary has to reconstruct.
fn line(m: &Msg) -> Option<String> {
    match m {
        Msg::System(_) => None,
        Msg::User(t) if t.trim().is_empty() => None,
        Msg::User(t) => Some(format!("[User]: {t}")),
        Msg::Assistant(t) if t.trim().is_empty() => None,
        Msg::Assistant(t) => Some(format!("[Assistant]: {t}")),
        Msg::AssistantCalls(raw) => Some(format!("[Assistant tool calls]: {}", clip(raw, SERIALISED_RESULT))),
        Msg::Tool { content, .. } => Some(format!("[Tool result]: {}", clip(content, SERIALISED_RESULT))),
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}\n[truncated]")
}

/// Render a slice of history as the text the summariser reads.
pub fn serialise(msgs: &[Msg]) -> String {
    msgs.iter().filter_map(line).collect::<Vec<_>>().join("\n")
}

/// Roughly how large a slice is on the wire.
fn width(msgs: &[Msg]) -> usize {
    msgs.iter().map(|m| m.width()).sum()
}

/// Whether a message may begin the preserved tail.
///
/// Only a user turn or an assistant's tool-call turn. A `Msg::Tool` starts
/// nothing: its `tool_call_id` refers to an `AssistantCalls` that would be
/// left behind, and a transcript that answers a call nobody made is
/// rejected by every provider that validates the pairing.
fn is_boundary(m: &Msg) -> bool {
    matches!(m, Msg::User(_) | Msg::AssistantCalls(_))
}

/// The index where the verbatim tail should start.
///
/// Walks boundaries newest-first and takes the oldest one whose suffix
/// still fits in `budget`. Returns `None` when even the newest turn is
/// bigger than the budget — in which case there is no tail to preserve and
/// the caller must not summarise, because it would be throwing away the
/// only turn the model is actually working on.
pub fn tail_start(msgs: &[Msg], budget: usize) -> Option<usize> {
    let mut best: Option<usize> = None;
    for i in (1..msgs.len()).rev() {
        if !is_boundary(&msgs[i]) {
            continue;
        }
        if width(&msgs[i..]) > budget {
            break;
        }
        best = Some(i);
    }
    best
}

/// How much of the transcript is kept word for word.
pub fn preserve_budget(context_budget: usize) -> usize {
    (context_budget / PRESERVE_FRACTION).clamp(PRESERVE_MIN, PRESERVE_MAX)
}

/// What a summarisation would do, worked out before any model is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Index of the first message kept verbatim. Everything from 1 to here
    /// is summarised; index 0 is the system prompt and is always kept.
    pub tail_start: usize,
    /// The request to send. Already carries the history and the template.
    pub prompt: String,
}

/// Decide whether and how to summarise.
///
/// `None` means do not: there is nothing old enough to be worth it, no tail
/// that fits, or the summarisation request would not fit either. Every one
/// of those is a reason to leave the transcript alone rather than to try
/// something clever.
pub fn plan(msgs: &[Msg], context_budget: usize, prior: Option<&str>) -> Option<Plan> {
    if msgs.len() < 4 {
        return None;
    }
    let tail = tail_start(msgs, preserve_budget(context_budget))?;
    // Nothing but the system prompt would be summarised.
    if tail <= 1 {
        return None;
    }

    // Everything the prompt carries besides the conversation itself. Room
    // for the summary comes off first: a compaction whose answer cannot fit
    // is not a compaction.
    let overhead = TEMPLATE.len()
        + FRAME
        + prior.map(|p| p.len() + UPDATE.len()).unwrap_or(0)
        + SUMMARY_ROOM;
    let room = context_budget.checked_sub(overhead)?;

    // How much of the head can be summarised in one request. Usually all
    // of it. On a very long run the serialised head is itself bigger than
    // the context, and opencode's answer there is to give up -- which
    // leaves the run sending an oversized request and dying of it, which
    // is exactly what happened to us twice. Summarising the oldest part
    // that *does* fit is strictly better: the transcript shrinks, the run
    // continues, and the next compaction folds this summary in and takes
    // another bite.
    let cut = head_cut(msgs, tail, room, prior)?;
    let head = head_slice(msgs, cut, tail);
    let conversation = serialise(&head);
    if conversation.trim().is_empty() {
        return None;
    }

    let mut prompt = format!("Here is the conversation so far:

<conversation>
{conversation}
</conversation>

");
    match prior {
        Some(p) if !p.trim().is_empty() => {
            prompt.push_str(&format!("Here is the summary of everything before that conversation:

<prior-summary>
{p}
</prior-summary>

"));
            prompt.push_str(UPDATE);
            prompt.push_str("

");
        }
        _ => prompt.push_str(
            "Write an anchored summary of the conversation above so that another agent can pick the work up from it alone.

",
        ),
    }
    prompt.push_str(TEMPLATE);

    Some(Plan { tail_start: cut, prompt })
}

/// The fixed text around the conversation and the template: the two
/// framing sentences and their tags. Measured generously rather than
/// computed, because being a few hundred characters pessimistic costs
/// nothing and being optimistic costs a 413.
const FRAME: usize = 600;

/// The messages that go into the summary: `msgs[1..cut]`, minus the
/// previous summary and the question it answered, which travel separately
/// as `<prior-summary>`. Summarising a summary alongside the conversation
/// would amplify whatever the last round got wrong.
fn head_slice(msgs: &[Msg], cut: usize, _tail: usize) -> Vec<Msg> {
    let skip = msgs.iter().position(|m| matches!(m, Msg::User(t) if t == QUESTION)).filter(|i| *i + 1 < cut);
    msgs[1..cut]
        .iter()
        .enumerate()
        .filter(|(off, _)| match skip {
            Some(i) => off + 1 != i && off + 1 != i + 1,
            None => true,
        })
        .map(|(_, m)| m.clone())
        .collect()
}

/// The largest boundary at or before `tail` whose serialised head fits in
/// `room`. `None` when not even the first turn fits, which means there is
/// nothing this can usefully do.
fn head_cut(msgs: &[Msg], tail: usize, room: usize, _prior: Option<&str>) -> Option<usize> {
    let mut best: Option<usize> = None;
    for c in 2..=tail {
        if c != tail && !is_boundary(&msgs[c]) {
            continue;
        }
        if serialise(&head_slice(msgs, c, tail)).len() > room {
            break;
        }
        best = Some(c);
    }
    // A cut of 2 summarises one message, which is churn rather than
    // compaction. Ask for at least a few.
    best.filter(|c| *c >= 4)
}

/// Rebuild a transcript around a summary.
///
/// The result is `[system, "What did we do so far?", summary, ...tail]`.
/// The question is opencode's, and it is there so the summary sits in the
/// transcript as an assistant turn answering something, rather than as a
/// system message the model may read as a new instruction.
pub fn apply(msgs: &[Msg], plan: &Plan, summary: &str) -> Vec<Msg> {
    let mut out = Vec::with_capacity(msgs.len() - plan.tail_start + 3);
    if let Some(sys @ Msg::System(_)) = msgs.first() {
        out.push(sys.clone());
    }
    out.push(Msg::User(QUESTION.into()));
    out.push(Msg::Assistant(summary.to_string()));
    out.extend_from_slice(&msgs[plan.tail_start..]);
    out
}

/// The user turn the summary answers.
pub const QUESTION: &str = "What have we done so far?";

/// Recover the summary already in a transcript, if it has been compacted
/// before, so the next summary folds it in instead of losing it.
pub fn prior(msgs: &[Msg]) -> Option<&str> {
    let i = msgs.iter().position(|m| matches!(m, Msg::User(t) if t == QUESTION))?;
    match msgs.get(i + 1) {
        Some(Msg::Assistant(s)) => Some(s.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: &str, n: usize) -> Msg {
        Msg::Tool { id: id.into(), content: "x".repeat(n) }
    }
    fn calls(id: &str) -> Msg {
        Msg::AssistantCalls(format!(r#"[{{"id":"{id}","type":"function","function":{{"name":"read","arguments":"{{}}"}}}}]"#))
    }

    fn long_run(steps: usize, size: usize) -> Vec<Msg> {
        let mut m = vec![Msg::System("sys".into()), Msg::User("goal".into())];
        for i in 0..steps {
            m.push(calls(&format!("c{i}")));
            m.push(tool(&format!("c{i}"), size));
        }
        m
    }

    #[test]
    fn the_tail_never_starts_on_an_orphaned_tool_result() {
        let msgs = long_run(40, 1_000);
        for budget in [1_000, 5_000, 20_000, 100_000] {
            if let Some(i) = tail_start(&msgs, budget) {
                assert!(
                    is_boundary(&msgs[i]),
                    "tail starts at {i} on {:?}, which orphans a tool result",
                    msgs[i]
                );
            }
        }
    }

    #[test]
    fn the_tail_fits_the_budget_and_is_as_long_as_it_can_be() {
        let msgs = long_run(40, 1_000);
        let budget = 12_000;
        let i = tail_start(&msgs, budget).expect("a tail should fit");
        assert!(width(&msgs[i..]) <= budget, "tail overflows its budget");
        // One boundary earlier would not have fitted, or the walk stopped early.
        let earlier = (1..i).rev().find(|j| is_boundary(&msgs[*j]));
        if let Some(j) = earlier {
            assert!(width(&msgs[j..]) > budget, "a longer tail was available at {j} and was not taken");
        }
    }

    #[test]
    fn a_turn_too_big_for_the_budget_yields_no_tail() {
        // One enormous step. There is nothing to preserve that fits, so
        // summarising would throw away the turn the model is mid-thought on.
        let msgs = vec![Msg::System("s".into()), Msg::User("g".into()), calls("c0"), tool("c0", 200_000)];
        assert_eq!(tail_start(&msgs, 10_000), None);
        assert_eq!(plan(&msgs, 40_000, None), None);
    }

    #[test]
    fn a_short_run_is_left_alone() {
        let msgs = vec![Msg::System("s".into()), Msg::User("g".into()), Msg::Assistant("hi".into())];
        assert_eq!(plan(&msgs, 240_000, None), None);
    }

    #[test]
    fn a_head_too_big_for_one_request_is_summarised_a_bite_at_a_time() {
        // opencode declines outright when the summarisation prompt would
        // itself overflow, which leaves the run sending an oversized
        // request and dying of it -- twice, in our own capability runs.
        // Summarising the oldest part that does fit is strictly better.
        let msgs = long_run(200, 2_000);
        let budget = 60_000;
        let p = plan(&msgs, budget, None).expect("should summarise what it can");
        assert!(p.prompt.len() < budget, "the request must fit: {} vs {budget}", p.prompt.len());
        assert!(p.tail_start > 1 && p.tail_start < msgs.len(), "it should take a bite, not all or nothing");

        // And it converges: applying it leaves a shorter transcript, which
        // the next pass bites into again.
        let once = apply(&msgs, &p, "## Objective
- first bite");
        assert!(once.len() < msgs.len());
        let p2 = plan(&once, budget, prior(&once)).expect("a second bite");
        let twice = apply(&once, &p2, "## Objective
- second bite");
        assert!(twice.len() < once.len(), "each pass must make progress");
    }

    #[test]
    fn a_budget_smaller_than_the_fixed_prompt_declines() {
        // There is no bite small enough. Leaving the transcript alone is
        // the only honest answer.
        let msgs = long_run(200, 2_000);
        assert_eq!(plan(&msgs, 10_000, None), None);
    }

    #[test]
    fn the_prompt_carries_the_history_and_the_template() {
        let mut msgs = long_run(30, 2_000);
        msgs.insert(1, Msg::User("build the scorecard".into()));
        let p = plan(&msgs, 400_000, None).expect("should summarise");
        assert!(p.prompt.contains("build the scorecard"), "the goal must reach the summariser");
        assert!(p.prompt.contains("## Next Move"), "the template must reach the summariser");
        assert!(p.prompt.contains("<conversation>"));
        assert!(!p.prompt.contains("<prior-summary>"), "there was no prior summary");
    }

    #[test]
    fn a_prior_summary_is_folded_in_rather_than_lost() {
        let msgs = long_run(30, 2_000);
        let p = plan(&msgs, 400_000, Some("## Objective\n- the original goal")).expect("should summarise");
        assert!(p.prompt.contains("<prior-summary>"));
        assert!(p.prompt.contains("the original goal"));
        assert!(p.prompt.contains("anything you do not carry into the new one is lost"));
    }

    #[test]
    fn applying_keeps_the_system_prompt_and_the_verbatim_tail() {
        let msgs = long_run(30, 2_000);
        let p = plan(&msgs, 400_000, None).expect("should summarise");
        let out = apply(&msgs, &p, "## Objective\n- did things");

        assert!(matches!(out[0], Msg::System(_)), "the system prompt must survive");
        assert!(matches!(&out[1], Msg::User(t) if t == QUESTION));
        assert!(matches!(&out[2], Msg::Assistant(s) if s.contains("did things")));
        assert_eq!(&out[3..], &msgs[p.tail_start..], "the tail must be word for word");
        assert!(out.len() < msgs.len(), "compaction should shrink the transcript");
    }

    #[test]
    fn the_rebuilt_transcript_still_pairs_calls_with_results() {
        let msgs = long_run(30, 2_000);
        let p = plan(&msgs, 400_000, None).expect("should summarise");
        let out = apply(&msgs, &p, "summary");
        // Every tool result must be preceded, somewhere earlier, by the
        // assistant turn that announced its id.
        for m in &out {
            let Msg::Tool { id, .. } = m else { continue };
            let announced = out.iter().any(|x| matches!(x, Msg::AssistantCalls(raw) if raw.contains(id)));
            assert!(announced, "tool result {id} has no matching call in the rebuilt transcript");
        }
    }

    #[test]
    fn a_summary_can_be_found_again_and_folded_into_the_next_one() {
        let msgs = long_run(30, 2_000);
        let p = plan(&msgs, 400_000, None).expect("should summarise");
        let out = apply(&msgs, &p, "## Objective\n- first pass");
        assert_eq!(prior(&out), Some("## Objective\n- first pass"));
        // And a transcript that was never compacted has none.
        assert_eq!(prior(&msgs), None);
    }

    #[test]
    fn the_preserved_slice_is_a_quarter_within_bounds() {
        assert_eq!(preserve_budget(240_000), 60_000);
        assert_eq!(preserve_budget(100_000), 25_000);
        assert_eq!(preserve_budget(9_000), PRESERVE_MIN);
        assert_eq!(preserve_budget(10_000_000), PRESERVE_MAX);
    }

    #[test]
    fn serialising_keeps_calls_and_their_answers_together() {
        let msgs = vec![Msg::User("goal".into()), calls("c1"), tool("c1", 10)];
        let text = serialise(&msgs);
        assert!(text.contains("[User]: goal"));
        assert!(text.contains("[Assistant tool calls]:"));
        assert!(text.contains("[Tool result]:"));
        // The system prompt is the harness talking to itself, not history.
        assert!(!serialise(&[Msg::System("rules".into())]).contains("rules"));
    }

    #[test]
    fn a_huge_tool_result_is_clipped_for_the_summariser() {
        let msgs = vec![tool("c1", 50_000)];
        let text = serialise(&msgs);
        assert!(text.contains("[truncated]"));
        assert!(text.len() < 5_000, "a single result should not fill the summarisation prompt");
    }
}
