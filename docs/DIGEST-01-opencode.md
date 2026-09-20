# DIGEST-01 — opencode Loops (sst-opencode @ depth-1, MIT)

## What to port (interface, not implementation)
- `packages/opencode/src/session/processor.ts` — `SessionProcessor.create()`: `while(true)` per-step LLM stream; pre-capture `snapshot.track()` before stream (AI SDK may exec tools pre-start-step); event switch: reasoning-start/delta/end, text-start/delta, tool-input-start(pending ToolPart)/tool-call(running + doom-loop check)/tool-result(completed)/tool-error(error, loop continues); finish-step token tally + compaction check; permission/question rejected -> blocked; abort/auth/overflow paths.
- `session/message-v2.ts` — parts: text/reasoning/tool/file/agent/compaction/snapshot/subtask/patch; tool part states pending/running/completed/error with input/output/metadata/title/time/attachments.
- `session/compaction.ts` — constants: PRUNE_MINIMUM 20_000, PRUNE_PROTECT 40_000, TOOL_OUTPUT_MAX_CHARS 2_000 (+`[truncated]`), PRUNE_PROTECTED_TOOLS ["skill"], recent window 2_000–15_000; turn-split, budget prune, summary marker, auto-trigger on overflow.
- `session/retry.ts` — exp backoff on 429/500/OutputLength/network; RetryPart history; max count.
- `session/revert.ts` — per-step patch + rollback (maps to the agent per-file undo scope).
- `permission/*` + doom-loop: last-3 same-tool+identical-args -> `doom_loop` ask gate, default ask.
- `tool/tool.ts` — `Def {id, description, parameters(Schema), execute(args,ctx)->{title,metadata,output,attachments}}`; `InvalidArgumentsError` -> model-facing rewrite prose. Context carries sessionID/messageID/agent/abort/ask/metadata.
- `tool/*.txt` — PROOF of per-tool embedded guidelines: e.g. `read.txt` = 14 lines usage + numbered rules (absolute paths, 2000-line window, offset paging, grep-first, parallel calls, truncation notes). the agent copies this shape: each of the 6 ops gets a `.txt`-style description with what + NOT + when + examples.

## Conventions worth adopting (repo AGENTS.md)
- Flat top-level exports + self-reexport, no `export namespace`, no barrel index in multi-sibling dirs (tree-shaking).
- Effect: `Effect.gen` compose, `Effect.fn("Domain.method")` traced, prefer FS/ChildProcess/HttpClient/Path/Clock services, `Effect.cached` dedup, `Effect.addFinalizer` cleanup.
- Style: no try/catch, no `any`, no else, early returns, functional array methods, Bun APIs, `bun typecheck` per package, no mocks in tests.
- Commits `type(scope): summary`, branches ≤3 words no slashes, default branch `dev`.

## Desktop Agent mapping
- Processor events -> the agent relay events (`step.start/live/done`, `xfer`, `paused`); snapshot.track/patch -> per-handle snapshot#; compaction budgets -> session memory budgets (protect `outline` like `skill`); doom-loop -> same-file same-op repeat gate; permission ask -> host approval UI.
