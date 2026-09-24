# 07 — opencode as a harness (backend digest)

Source: `sst/opencode` at `ebb7b76` (2026-09-19), cloned to `.tmp/repos/opencode`.
Method: read the files, one at a time, and write down what each one decides.
Nothing is ported here. §12 says exactly what was read and what was not.

This supersedes nothing: `DIGEST-06` covered the loop's control flow. This one
covers the **request**: how a turn is assembled, transformed per provider, and
recovered when it goes wrong. That is the half we were missing.

---

## 1. The loop, exactly (`session/prompt.ts`, 1,631 lines, read in full)

`runLoop` is a `while (true)` over the whole conversation, re-read from the
database at the top of every pass. It never holds the transcript in a local
variable across iterations — every step begins with
`MessageV2.filterCompactedEffect(sessionID)`.

Order of business inside one pass:

1. `status.set(busy)`.
2. Re-read messages; find `latest()` → `{user, assistant, finished, tasks}`.
3. **Exit test** (below).
4. `step++`. On step 1, fork title generation.
5. Pop a queued *task* — a subtask or a compaction — and handle it, then
   `continue`. Tasks are parts on a user message, not a side queue.
6. If the previous assistant's **real token usage** overflowed, queue a
   compaction and `continue`.
7. Build the assistant message, resolve tools, apply reminders, call the model
   through the processor, and act on its verdict.

The exit test is the part worth copying verbatim:

```ts
if (lastAssistant?.finish
    && !["tool-calls", "unknown"].includes(lastAssistant.finish)
    && !hasToolCalls
    && lastAssistant.parentID === lastUser.id) break
```

`hasToolCalls` exists because **some providers return `finish: "stop"` on a
turn that contains tool calls.** opencode does not believe the finish reason
over the content: if there are unanswered tool calls, the loop keeps going.
We found the mirror-image defect ourselves (`finish_reason: "tool_calls"`
with no calls in the body) and fixed only that direction. Both directions are
the same rule: *the parts decide, not the label.*

`isOrphanedInterruptedTool` excludes tool parts that `cleanup()` marked
`{status: "error", metadata.interrupted: true}` after a retry or an abort.
Without that exclusion an abandoned call would keep the loop alive forever.

### The step budget ends with a summary, not a stop

`maxSteps = agent.steps ?? Infinity`, and it is **per agent**, not global. On
the last step the messages sent are `[...history, {role: "assistant", content:
MAX_STEPS_PROMPT}]` — an *assistant-role prefill*, not a user message:

> CRITICAL - MAXIMUM STEPS REACHED … Tools are disabled until next user
> input. Respond with text only. … Response must include: statement that
> maximum steps have been reached; summary of what has been accomplished;
> list of remaining tasks; recommendations for what should be done next.

Our budget just stops. A run that dies at step 300 leaves the user nothing.
This is a cheap, high-value port: one constant and one appended message.

### Attachments enter the transcript as fake tool calls

When a user attaches a file, `createUserMessage` does not put a blob in the
prompt. It runs the real `read` tool and pushes **synthetic text parts**:

```
Called the Read tool with the following input: {"filePath":"…"}
<the tool's actual output>
```

The model sees its own prior read. Same for directories and MCP resources.
Binary attachments are gated: unsupported mime → a bracketed note; over 10 MB
→ a bracketed note with the size. It never silently drops.

### Title generation

Forked at step 1, on the provider's **small** model, from the first real user
message only. The response is cleaned with
`.replace(/<think>[\s\S]*?<\/think>\s*/g, "")` — reasoning models leak think
tags into a 6-word title — then the first non-empty line, capped at 100 chars
with an ellipsis. It runs `Effect.ignore`: a failed title never fails a run.

---

## 2. One step of streaming (`session/processor.ts`, 732 lines, read in full)

The processor owns a single assistant message and consumes one event stream.

**Retry wraps the whole stream**, not a request:

```ts
stream.pipe(Stream.tap(handleEvent), Stream.takeUntil(() => ctx.needsCompaction), Stream.runDrain)
  .pipe(Effect.onInterrupt(...), Effect.retry(SessionRetry.policy(...)), Effect.catch(halt), Effect.ensuring(cleanup()))
```

Three things follow from that shape:

- A retry **replays the stream from the beginning**, so partial parts written
  by the failed attempt have to be cleaned up. That is what `cleanup()` is
  for, and why interrupted tool parts get `metadata.interrupted = true` —
  which is then what the loop's exit test has to exclude. The three pieces
  are one design.
- `Stream.takeUntil(() => ctx.needsCompaction)` **cuts the stream off
  mid-generation** when `step-finish` reports usage over the limit. Overflow
  does not wait for the turn to end.
- `Effect.ensuring(cleanup())` runs after retries are exhausted, so every
  dangling tool call is settled exactly once.

**Return value** is `"compact" | "stop" | "continue"`:

- `compact` — usage overflowed, or the provider raised `ContextOverflowError`.
- `stop` — `ctx.blocked` (a permission was denied and
  `experimental.continue_loop_on_deny` is not set) or the message has an error.
- `continue` — otherwise.

**The doom loop.** On every `tool-call` event, the last `DOOM_LOOP_THRESHOLD =
3` parts of the current assistant message are checked: all three must be tool
parts, same tool name, `status !== "pending"`, and `JSON.stringify(input)`
byte-identical. Then it **asks** the human, with `always: [toolName]` as an
option — it does not abort. Our gate aborts, which is what ended arm A of
experiment 4 at step 87 with no way for a human to wave it through.

**Other things the event handler does** that we do not: emits `step-start` /
`step-finish` parts carrying a filesystem snapshot id and a diff patch;
accumulates cost and tokens per step; logs when Anthropic reports
`inputTransformations` (thinking blocks it dropped because history changed
behind a signed block); and surfaces `finish: "content-filter"` as a visible
error instead of an empty idle turn.

---

## 3. Compaction is three mechanisms (`session/compaction.ts` + `core/session/compaction.ts`)

`DIGEST-06` said two. There are three, and we have one.

### (a) Prune — erase old tool output in place

Ported already. Confirmed details: walks messages newest→oldest, skips the
**two most recent turns** (`if (msg.info.role === "user") turns++; if (turns <
2) continue`), stops at a summary message or at an already-compacted part,
accumulates `Token.estimate(part.state.output)`, and once past
`PRUNE_PROTECT = 40_000` marks the rest `state.time.compacted = Date.now()`.
Only commits if the total freed exceeds `PRUNE_MINIMUM = 20_000`.
`PRUNE_PROTECTED_TOOLS = ["skill"]`. A pruned part renders as
`"[Old tool result content cleared]"` — the call and its arguments stay.

It runs **forked, after the loop ends**, not inline. Housekeeping between
turns, not an emergency measure.

### (b) Summarise with a preserved verbatim tail

`select()` computes `preserveRecentBudget = cfg.compaction.preserve_recent_tokens
?? clamp(floor(usable * 0.25), 2_000, 15_000)`, walks turns newest→oldest
accumulating estimates until the budget is spent, and when a whole turn will
not fit calls `splitTurn()` — which scans *forward* inside that turn for the
earliest message whose suffix still fits. The result is `{head, tail_start_id}`:
head gets summarised, tail survives word for word.

The summary prompt (`core/session/compaction.ts`, fully portable) is a fixed
Markdown template:

```
## Objective
## Important Details
## Work State
### Completed / ### Active / ### Blocked
## Next Move
## Relevant Files
```

with rules: keep every section even when empty, terse bullets, **preserve
exact file paths, symbols, commands, error strings, URLs and identifiers**,
and never mention that compaction happened. When a prior summary exists,
`SUMMARY_UPDATE_INSTRUCTIONS` is added, and it states the stake plainly:
*"The prior summary is discarded after this: anything you do not carry into
the new summary is lost."* Plus a conflict rule — the conversation is newer
than the summary, so where they disagree the conversation wins and the old
claim is dropped.

A guard worth stealing: `if (Token.estimate(summaryPrompt) > context -
summaryOutput) return false`. If the summarisation request would itself
overflow, do not send it. Summary output is capped at 4,096 tokens.

### (c) Reorder, so the model reads summary-then-tail

`MessageV2.filterCompacted` (in `message-v2.ts`) does the assembly. It walks
newest→oldest until it finds a *completed* compaction, notes its
`tail_start_id`, keeps going to that message, then reverses — and finally
**reorders** to:

```
[compaction-user, summary-assistant, …retained tail…, everything after]
```

The compaction user message renders to the model as the literal text
`"What did we do so far?"`. So the model sees a question, its own answer (the
summary), and then the last quarter of the real conversation verbatim. There
is a comment warning that after this, array position is no longer
chronological — `latest()` sorts by `time.created` with the id as tie-break.

After an auto-compaction the loop does not just continue: it appends a
synthetic user part, *"Continue if you have next steps, or stop and ask for
clarification if you are unsure how to proceed."* If the compaction was
triggered by oversized media, the original user turn is **replayed** with the
media replaced by `[Attached image/png: chart.png]` text.

---

## 4. Request assembly (`session/llm/request.ts`, 226 lines, read in full)

The most directly portable file in the repo. `prepare()` builds:

**System.** `system[0] = (agent.prompt ?? SystemPrompt.provider(model)) + env
+ instructions + mcp + skills`, joined. Then, after the plugin hook, if there
are more than two entries they are collapsed back to two. That is not
cosmetic: caching marks the **first two** system messages, so the prompt is
kept to two blocks on purpose.

**Params.** `temperature` only if `model.capabilities.temperature`, else
omitted entirely; `topP`, `topK`, `maxOutputTokens = min(model.limit.output,
32_000) || 32_000`.

**Tools** are sorted alphabetically before sending — a deterministic order is
a cacheable order.

Two compatibility hacks worth knowing:
- OpenAI-family providers get `strict: false` stamped on every function tool,
  so MCP and dynamic schemas that fail structured-output constraints still
  register.
- GitHub Copilot rejects a request that replays tool calls with an empty
  `tools` field, so a `_noop` tool is injected whose description tells the
  model never to call it.

**Headers.** `x-session-affinity` and `X-Session-Id` on every request — a hint
to the provider's load balancer to land follow-ups on a warm cache.

---

## 5. Per-provider transformation (`provider/transform.ts`, 1,909 lines)

The file `DIGEST-06` admitted it had not read. It is a catalogue of
things that break in production. The portable parts:

**Message normalisation** (`normalizeMessages`):

| Provider | What breaks | What is done |
|---|---|---|
| all | lone surrogates in text crash serialisation | `sanitizeSurrogates` on every text, system, tool result |
| Anthropic, Bedrock | empty content is rejected | drop empty text/reasoning parts, then drop messages left empty |
| Claude | tool-call ids outside `[A-Za-z0-9_-]` | scrub |
| Mistral | tool-call ids must be exactly 9 alphanumerics | strip, truncate to 9, pad with `0` |
| Mistral | a tool message may not be followed by a user message | insert `assistant: "Done."` between them |
| DeepSeek | every assistant message must carry reasoning | append an empty reasoning part |
| interleaved-reasoning models | reasoning must ride on the message | move joined reasoning text into `providerOptions.openaiCompatible[field]`, always set even when empty |

**Caching** (`applyCaching`): four breakpoints — the first two `system`
messages and the last two non-system messages. Anthropic and Bedrock take the
marker at message level; everyone else on the last content part. The marker
name differs per provider (`cacheControl`, `cachePoint`, `cache_control`,
`copilot_cache_control`) but the placement rule is one rule.

**Unsupported media** (`unsupportedParts`): rather than erroring, the
attachment is replaced by a text part — `"ERROR: Cannot read "chart.png"
(this model does not support image input). Inform the user."` The refusal is
addressed to the model, so the model can explain it. Empty base64 gets its own
message. This is the same instinct as our refuse-don't-truncate rule, applied
to inputs.

**Schema rewriting** (`schema`), applied to every tool before it is sent:
- OpenAI/Azure: rebuild the schema keeping only known keywords; JSON Schema's
  boolean form becomes `{type: "string"}`; `const` becomes a one-element
  `enum`; a missing `type` is **inferred** from which keywords are present
  (`properties` ⇒ object, `items` ⇒ array, `format`/`enum` ⇒ string, numeric
  bounds ⇒ number) so a stripped schema stays usable.
- Moonshot/Kimi: a node with `$ref` may carry no siblings, so it is reduced to
  `{$ref}` alone; tuple-form `items` arrays collapse to their first element.
- Google/Gemini: integer enums become string enums (and the type with them);
  a `type` array becomes `anyOf` with `nullable` lifted out; `required` is
  filtered to fields that actually exist in `properties`; `properties` and
  `required` are deleted from non-object types.

That last one is the lesson for us. Our closed-schema rules
(`additionalProperties: false`, a cap on every string) are correct *for a
model that accepts them*. The surface has to be rewritten per provider, at the
wire, and `tools.rs` is not where that belongs.

**`options()`** is a long per-model table of request options: `store: false`
for OpenAI-family, `usage: {include: true}` for OpenRouter, thinking
configuration per family. Not portable as code, but the shape — one function
mapping model id to request options — is.

---

## 6. Recovering a malformed tool call (`session/llm.ts` + `tool/invalid.ts`)

`experimental_repairToolCall` runs when a tool call fails validation:

1. If the name differs from a real tool **only by case**, lowercase it and let
   it through.
2. Otherwise rewrite the call to a tool literally named `invalid`, with
   `{tool: <the bad name>, error: <the message>}` as its input.

`invalid` is a real registered tool whose description is `"Do not use"` and
whose output is `"The arguments provided to the tool are invalid: …"`. It is
excluded from `activeTools`, so the model is never offered it — but every
malformed call still has somewhere to land, and the model gets a tool result
explaining what was wrong instead of a dead turn.

This is the general form of the defect that produced 29 of 63 refusals in one
of our runs: a provider mangled `function.name`, and our harness had no way to
answer a call it could not name. A `invalid` sink plus a case-insensitive
retry would have absorbed all of it.

Note also `maxRetries: input.retries ?? 0` — the SDK's own retry is **off**.
Retry belongs to one layer (the processor), not two.

---

## 7. The tool surface changes per model (`tool/registry.ts`, `session/tools.ts`)

`registry.tools({providerID, modelID, agent, permission})` filters before any
prompt is built:

```ts
const usePatch = modelID.includes("gpt-") && !modelID.includes("oss") && !modelID.includes("gpt-4")
if (tool.id === ApplyPatchTool.id) return usePatch
if (tool.id === EditTool.id || tool.id === WriteTool.id) return !usePatch
```

**GPT models get a different editing verb than everyone else.** Not a
different prompt about the same verb — a different tool. Web search is only
offered on providers that have it. `execute` (code mode) only appears if
there is an MCP catalog to describe.

Tool descriptions are also assembled per call: the `task` tool's description
has the list of available subagents appended, filtered by permission and
sorted by name.

Then `session/tools.ts` wraps each one with `ProviderTransform.schema(model,
…)` (§5) and a context carrying `ask` (permission) and `metadata` (progress
updates that stream into the running tool part).

---

## 8. Per-model-family system prompts (`session/system.ts`, read in full)

Selection is a substring ladder on `model.api.id`, first match wins:

| Test | Prompt |
|---|---|
| `muse` | `meta.txt` with `{{MODEL_NAME}}` substituted |
| `gpt-4`, `o1`, `o3` | `beast.txt` |
| `gpt-6` | `gpt-astra.txt` |
| `gpt` + `codex` | `codex.txt` |
| `gpt` | `gpt.txt` |
| `gemini-` | `gemini.txt` |
| `claude` | `anthropic.txt` |
| `trinity` | `trinity.txt` |
| `kimi`, or moonshot providers | `kimi.txt` |
| — | `default.txt` |

An agent with its own `prompt` overrides all of it (`request.ts:60`), which is
how `explore`, `compaction`, `title` and `summary` work.

`environment()` adds a block we should copy almost verbatim, because it is
cheap and it answers questions the model otherwise guesses at:

```
You are powered by the model named <id>. The exact model ID is <provider>/<id>
<env>
  Working directory: …
  Workspace root folder: …
  Is directory a git repo: …
  Platform: …
  Today's date: …
</env>
```

For us the equivalent is: which applications are attached, which documents are
open, which handles exist, and today's date.

---

## 9. Agents, not routes (`agent/agent.ts`)

An agent is `{name, description, mode: subagent|primary|all, model?, variant?,
prompt?, temperature?, topP?, options, permission: Ruleset, steps?, hidden?}`.
Four are built in and hidden: `explore`, `compaction`, `title`, `summary` —
each with `"*": "deny"` permission and its own prompt file.

This is a different axis from our `Model::{Small, Standard, Coding,
Reasoning}` router. Ours picks a model by job size. Theirs bundles *prompt +
permission + model + step budget* under a name, and the small-model choice
falls out of that. The compaction agent being an agent is what lets it
run on a different model than the conversation.

---

## 10. Status and run state (`session/status.ts`, `session/run-state.ts`)

The whole externally-visible state is `idle | busy | retry{attempt, message,
action, next}`. `retry` carries the next wake time and an optional `action`
with a title, a label and a link — that is how "rate limited, retrying in 8s"
renders without the UI knowing anything about providers.

`run-state.ts` keeps one runner per session. `ensureRunning` joins an existing
run rather than starting a second; `startShell` fails with `BusyError` instead.
Cancelling a session also cancels background jobs transitively — jobs whose
metadata names the session, then jobs whose metadata names *those* jobs, until
the set stops growing.

---

## 11. Port status

| # | Item | State |
|---|---|---|
| 1 | retry with jitter + `Retry-After` | done |
| 2 | prune by token budget | done |
| 3 | doom-loop detection | done as an abort; **should ask, with `always`** |
| 4 | five broken-reply defects | done |
| 5 | per-slot providers + auth resolution | done |
| 6 | exit on parts, not on finish reason | **half** — we handle calls-without-body, not stop-with-calls |
| 7 | `MAX_STEPS_PROMPT` summary at the budget | **not done** — cheapest high-value item |
| 8 | summarise with preserved tail + reorder | **not done** — the big one |
| 9 | per-model-family system prompts | **not done** — the other big one |
| 10 | `<env>` block in the system prompt | **not done** — cheap |
| 11 | `invalid` tool sink + case-insensitive repair | **not done** — cheap, and it is our worst measured defect |
| 12 | per-provider schema rewriting at the wire | **not done** |
| 13 | cache breakpoints (first 2 system, last 2 messages) | **not done** |
| 14 | tool surface varying by model | **not done** — decide deliberately |
| 15 | attachments as synthetic read-tool calls | **not done** |
| 16 | status as `idle/busy/retry` with a next-wake time | **not done** — needed by the UI |

Recommended order: 11, 7, 10 (a day, and each is independently useful), then
6, then 8 and 9.

---

## 12. Coverage of this digest

**Read in full:** `session/prompt.ts` (1,631), `session/processor.ts` (732),
`session/compaction.ts` (608), `core/session/compaction.ts` (248),
`session/llm/request.ts` (226), `session/system.ts` (154),
`session/run-state.ts` (151), `session/status.ts` (56),
`session/overflow.ts` (34), `tool/invalid.ts` (20).

**Read in the parts that decide something:** `provider/transform.ts` (~700 of
1,909 — `normalizeMessages`, `applyCaching`, `unsupportedParts`, `message`,
`options`, `schema`, `maxOutputTokens`; the unread middle is the
reasoning-effort/variant catalogue per model id), `session/message-v2.ts`
(~300 of 737 — `toModelMessagesEffect`, `filterCompacted`, `latest`,
`fromError`), `session/llm.ts` (~330 of 404 — `prepare` call site,
`repairToolCall`, the stream adapter), `tool/registry.ts` (~200 of 455 —
`tools()`, `describeTask`), `session/tools.ts` (~150 of 590 — `resolve` and
the tool wrapper; the rest is MCP resource tools), `agent/agent.ts` (~120 of
453 — the `Info` schema and the built-in agents), `session/retry.ts` (~120 of
209, and it is already ported), `session/summary.ts` (~90 of 160 — it turned
out to be about file diffs, not conversation summaries).

**Not read:** `provider/provider.ts` (2,072 — model catalogue and SDK
loading, not portable to a zero-dependency Rust core), `session/session.ts`
(1,016 — session CRUD over SQL), `session/instruction.ts` (237),
`session/revert.ts` (136), `mcp/index.ts` (1,004), `tool/edit.ts` (737),
`tool/shell.ts` (645), `snapshot/index.ts` (807), `config/config.ts` (701),
the nine prompt `.txt` files (1,302 lines total — read as a set in
`DIGEST-06`, not re-read here).

The one that could still change a conclusion is `provider/provider.ts`, for
how a model's `limit.context`, `limit.output` and `capabilities` are
populated — every budget in §3 and §4 is computed from those, and we
currently hard-code them.
