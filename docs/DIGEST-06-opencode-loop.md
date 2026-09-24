# DIGEST-06 — opencode's agentic loop

> Companion: `DIGEST-07-opencode-harness.md` covers the request half —
> assembly, per-provider transformation, and recovery. Read together.

Source: `sst/opencode` @ `ebb7b76`, shallow clone in `.tmp/repos/opencode`.
Scope: `packages/opencode/src/session/` (8,117 lines) and
`packages/opencode/src/provider/` (4,413 lines).

`docs/08` claimed these loops were ported. Reading them properly, the
*shape* was ported and most of the mechanism was not. §11 states exactly
which files were read for this digest and which were not, so the coverage
is checkable rather than implied.

---

## 1. The outer loop — `prompt.ts:1088`

```
while (true):
  status = busy
  msgs = filterCompacted(session)
  (lastUser, lastAssistant, lastFinished, tasks) = latest(msgs)

  hasToolCalls = lastAssistantMsg.parts.some(
      part.type == "tool" && !providerExecuted && !orphanedInterrupted)

  if lastAssistant.finish
     and finish not in ["tool-calls", "unknown"]
     and not hasToolCalls
     and lastAssistant.parentID == lastUser.id:
        break                          # the ONLY normal exit

  step++
  if step == 1: fork a title-generation job
  if task is subtask     -> handle, continue
  if task is compaction  -> process; "stop" -> break; else continue
  if isOverflow(lastFinished.tokens, model) -> create compaction, continue

  agent    = agents.get(lastUser.agent)
  maxSteps = agent.steps ?? Infinity
  isLastStep = step >= maxSteps
  msgs = SessionReminders.apply({messages, agent, session})
  handle = processor.create({assistantMessage, sessionID, model})
```

**The rule we do not have** (`prompt.ts:1102`, comment verbatim):

> Some providers return "stop" even when the assistant message contains
> tool calls. Keep the loop running so tool results can be sent back to the
> model, but ignore cleanup-marked interrupted orphans.

They exit on a *finish reason*, and then only when there are genuinely no
tool calls **and** the assistant message answers the current user message.
We exit on "the reply had no tool calls and some prose", which is a weaker
test of the same thing and is why narration ended three of our runs.

**Overflow is checked before the model is called.** We learn about overflow
as a `413` from the provider, after the fact.

**`maxSteps` is per-agent and defaults to Infinity.** Ours is a global
`AGENT_MAX_STEPS`, default 40.

---

## 2. The processor — `processor.ts`

`Result = "compact" | "stop" | "continue"`, decided at `processor.ts:693`:

```
if ctx.needsCompaction            return "compact"
if ctx.blocked || message.error   return "stop"
return "continue"
```

Tool calls move through explicit states, each one persisted as a *part*:

```
tool-input-start  -> pending
tool-call         -> running   (+ doom-loop check)
tool-result       -> completed
tool-error        -> error, and the loop CONTINUES
```

A tool error does not end the run. Ours refuses and continues too, which
matches — but ours has no `pending` state, so a call that never returns is
invisible rather than visibly stuck.

**Image attachments are normalised and, on failure, omitted with a
counted note** appended to the output: `[N images omitted: could not be
resized below the image size limit.]` The principle is one we share and
should keep sharing — say what was dropped, never drop silently.

---

## 3. The doom-loop gate — `processor.ts:29, 352`

```
DOOM_LOOP_THRESHOLD = 3
recentParts = parts.slice(-3)
fires only when all three are:
   part.type == "tool"
   part.tool == value.name
   part.state.status != "pending"
   JSON.stringify(part.state.input) == JSON.stringify(input)
-> permission.ask({permission: "doom_loop", patterns:[tool],
                   always:[tool], ruleset: agent.permission})
```

Same tool **and byte-identical input**, three in a row — and it **asks**,
with an `always` option so a human can wave that tool through for the rest
of the session. Ours stops the run outright. Arm A of experiment 4 died
exactly here: it retried a read three times because the document had been
closed, and the gate killed a run that a human could have unblocked.

---

## 4. Retry — `retry.ts`

```
RETRY_INITIAL_DELAY   = 2000 ms      RETRY_JITTER_FACTOR = 0.25
RETRY_BACKOFF_FACTOR  = 2            RETRY_MAX_RETRIES   = 5
RETRY_MAX_DELAY_NO_HEADERS = 30_000

delay(attempt, error):
    headers["retry-after-ms"]                     -> that, capped
    headers["retry-after"] (seconds or HTTP date) -> that, capped
    headers present but neither                   -> exponential, capped
    no headers                                    -> min(exponential, 30s)

exponential(n) = base + base*0.25*random,  base = 2000 * 2^(n-1)
```

Retryable (`retry.ts:32`), seven regexes: status codes `429|500|502|503|504|524`;
rate-limit phrasings; capacity phrasings (`overloaded`, `service unavailable`,
`internal error`, `provider returned error`); network failures (`terminated`,
`fetch failed`, `upstream connect`, `socket hang up`, `getaddrinfo`,
`enotfound`, `eai_again`, `econnrefused`, `econnreset`, `etimedout`);
timeout phrasings; `try your request again` / `resource exhausted`;
`try again later` / `currently at capacity`.

Plus two rules in `retryable()`: **any 5xx is retried even when the SDK did
not mark it retryable**, and **context overflow is never retried**
(`retry.ts:87`). They also special-case free-tier and account limits into a
user-facing action with a link, which is product, not mechanism.

**Ported** (see §10): the curve, the jitter, five attempts, the patterns.
**Not ported**: `Retry-After`. `send_via_curl` discards response headers,
so honouring it needs `curl -D` and a second stream to parse. The function
exists and is tested; it has no caller.

---

## 5. Overflow — `overflow.ts`

```
COMPACTION_BUFFER = 20_000
reserved = cfg.compaction.reserved ?? min(20_000, maxOutputTokens(model))
usable   = model.limit.input ? max(0, limit.input - reserved)
                             : max(0, limit.context - maxOutputTokens)
count    = tokens.total || input + output + cache.read + cache.write
overflow when count >= usable
```

Real token counts from the provider's usage block, against that model's own
limit, including **cache read and write tokens** — which a naive count
misses and which dominate on a long cached conversation.

Ours is a flat character estimate that knows nothing about the model. **We
do not parse `usage` at all.**

---

## 6. Compaction is TWO mechanisms, not one

My first pass found one and assumed it was the whole thing.

### 6a. Prune — `compaction.ts:273`

```
PRUNE_MINIMUM = 20_000 tokens    PRUNE_PROTECT = 40_000 tokens
TOOL_OUTPUT_MAX_CHARS = 2_000    PRUNE_PROTECTED_TOOLS = ["skill"]

walk messages NEWEST -> OLDEST:
    count user messages as turns; skip while turns < 2
    stop at an assistant summary
    for each completed tool part, newest first:
        skip protected tools
        stop at an already-compacted part
        total += estimate(output)
        if total <= PRUNE_PROTECT: keep
        else: mark
if pruned > PRUNE_MINIMUM: apply, stamping state.time.compacted
```

Protect a **token budget** and a **turn**, not a message count; do nothing
unless the saving is worth the churn; mark rather than rewrite.

### 6b. Summarise — `compaction.ts:114-190, 223, 319, 559`

When a session overflows, prune is not the answer — summarisation is.

```
preserveRecentBudget = cfg.compaction.preserve_recent_tokens
                    ?? clamp(floor(usable * 0.25), 2_000, 15_000)

turns(messages)   -> slice into turns, each bounded by a user message,
                     skipping any user message that is itself a compaction

splitTurn(turn, budget) -> walk FORWARD from turn.start+1; return the
                     earliest index whose slice to turn.end still fits the
                     budget. That is the largest verbatim tail that fits.

select() -> choose the turn to cut at, keep that Tail
process() -> summarise everything before it into a summary message
create() -> enqueue a compaction task the outer loop picks up
```

So the history becomes **summary + the most recent ~25% of the window kept
word for word**, cut at a turn boundary so no turn is half-summarised.

**We have nothing like this.** We truncate old tool results and the early
context is simply lost, with no summary standing in for it. On a long run
that is the difference between a model that forgets what it decided and one
that can still read a précis of it.

---

## 7. Per-model-family system prompts — `system.ts:28`

**The single largest gap, and my first pass missed it completely.**

```
model.api.id contains "muse"        -> meta.txt      (name substituted)
  "gpt-4" | "o1" | "o3"             -> beast.txt
  "gpt-6"                           -> gpt-astra.txt
  "codex"                           -> codex.txt
  "gpt"                             -> gpt.txt
  "gemini-"                         -> gemini.txt
  "claude"                          -> anthropic.txt
  "trinity"                         -> trinity.txt
  "kimi" | moonshot providers       -> kimi.txt
  otherwise                         -> default.txt
```

Nine prompts, 46 to 155 lines each, 1,302 lines of prompt text in total.
They are not cosmetic variants: `default.txt` spends a long paragraph
capping verbosity with worked examples, `anthropic.txt` opens by naming the
model and uses terse bullets, `gpt-astra.txt` is 46 lines where `beast.txt`
is 147.

**We send one system prompt to every model.** Our capability runs used four
different families — nemotron, ling, qwen, deepseek — through one prompt
written without any of them in mind. opencode's structure says that is a
mistake worth fixing, and it is cheap to fix: the selection is a substring
match on the model id.

`system.ts` also assembles, per turn: `environment` (cwd, platform, git
state), `skills`, and an `mcp` section listing available MCP resources.

---

## 8. Reminders — `reminders.ts`

Per-turn guidance is appended as a **synthetic text part on the last USER
message**, `synthetic: true`, not as a system message:

```
userMessage.parts.push({type:"text", text: PROMPT_PLAN, synthetic: true})
```

`synthetic` means the UI does not show it to the human but the model
receives it. It is recomputed every loop iteration, so it is always current.

Ours is a self-replacing system message at slot 1 carrying the registry,
the plan and the budget. Same intent; different attachment point. Theirs
rides with the user turn, which is where models weight instructions most
heavily — worth testing against ours rather than assuming either is better.

---

## 9. Plan and todo are two separate things

**`todo.ts`** — a structured task list in SQL:

```
{content, status, priority, position}
status: pending | in_progress | completed | cancelled
update() DELETEs the whole list and re-INSERTs   (replace, never merge)
then publishes Event.Updated so the UI re-renders
```

**Plan mode** — a separate *agent* (`agent.name === "plan"`) plus a plan
**file on disk** that the model writes with the ordinary `write` tool. On
switching back to `build`, a reminder is injected telling it to execute the
plan at that path.

Our `plan` tool sits between the two: a list like `todo`, but with only
done/not-done where theirs has four states including `in_progress` and
`cancelled`, no priority, and no event for a UI to render. Replace-the-whole-
list matches. Worth taking: `in_progress` (which is how a reader can see
where a run actually is) and the event.

---

## 10. Port status

| # | Item | State |
|---|---|---|
| 1 | retry curve, jitter, 5 attempts | **ported** |
| 2 | the seven retryable patterns | **ported** |
| 3 | prune by token budget + turn + minimum | **ported**, with one adaptation |
| 4 | protected tools (`manual`, `plan` for theirs `skill`) | **ported** |
| 5 | auth.json: api / oauth / wellknown, env override, slash normalising | **ported** |
| 6 | `Retry-After` honoured | written, **no caller** — needs `curl -D` |
| 7 | parse `usage`, overflow against the model's real limit | **not done** |
| 8 | summarise-with-tail-preservation | **not done** — the big one |
| 9 | per-model-family system prompts | **not done** — the other big one |
| 10 | exit on finish reason, not on "prose happened" | **not done**; changes scored behaviour, lands with v4 |
| 11 | doom loop asks with `always` instead of stopping | **not done** |
| 12 | todo `in_progress`/`cancelled` + an update event | **not done** |
| 13 | reminders as synthetic user parts | **not done**; test against our system-slot note |

**The adaptation in 3, stated because it is a real divergence:** opencode
counts a turn as a user message, because a human speaks between turns. An
autonomous run has one goal and then hundreds of assistant/tool pairs, so
counting their way protected the entire transcript and pruned nothing — I
measured that before changing it: 407,013 characters, zero pruned. Our turn
boundary is one assistant step.

---

## 11. Coverage of this digest

Read in full: `overflow.ts` (34), `reminders.ts` (92), `todo.ts` (74),
`retry.ts` (209), `auth/index.ts` (229, for DIGEST-07).

Read in the parts that matter: `prompt.ts` (the loop, 1088-1250 of ~1,900),
`processor.ts` (doom loop, tool states, the Result decision — ~200 of 732),
`compaction.ts` (prune, turns, splitTurn, the interface — ~250 of 608),
`system.ts` (prompt selection and the interface — ~60 of 154),
`tools.ts` (how the surface is resolved per agent — ~40 of 590),
`run-state.ts` (the interface — ~50 of 151).

**Not read:** `provider/transform.ts` (1,909 lines — per-provider request
quirks, `maxOutputTokens`, caching, message rewriting), `message-v2.ts`
(737 — the part model and `filterCompacted`), `provider/provider.ts` (2,072
— the model catalog and SDK loading, mostly not portable), `llm.ts` and
`llm/`, `instruction.ts` (237), `summary.ts` (160), `session.ts`,
`revert.ts`, `message-error.ts`, `schema.ts`.

`transform.ts` is the largest unread piece and the most likely to contain
something we need, because it is where "this provider needs the request
shaped differently" lives — which is exactly the problem a multi-provider
fallback chain runs into.
