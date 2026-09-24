# 08 — Production-Grade: Single-User Device, Porting + Digestion Protocol

Date: 2026-09-10
Status: production-grade baseline

## Scope lock
- Single user, single device (Claude Desktop / ChatGPT Desktop shape). No multi-tenant, no org graph, no Entra. One Windows machine + Office installed = full system.
- Windows + MS Office POC first. Linux/Mac in CI later (same Rust core, hands swap to file-level + Office.js).
- Production bar: clippy -D warnings, tests on Win/Linux/Mac, no orphan processes, signed bundle, crash backoff, port conflicts handled.

## Best-practices stack (efficient widget)
- Tauri v2 Rust + WebView2, Preact/Svelte widget (~8MB, ~40MB idle). Always-on-top drag, hotkey, transparency.
- Sidecar pattern: Tauri `externalBin` + `shell:allow-execute/spawn` (tauri.conf capabilities). C# STA `office-host.exe` for COM (message pump, IOleMessageFilter busy retry, modal detection, soft timeouts). Named pipe `office-rpc/1` local; wss relay only for remote panes.
- Lifecycle: supervisor per sidecar (spawn, health TCP/HTTP/stdout-marker, auto-restart backoff, graceful SIGTERM->kill tree via Job Objects Win / process groups Unix, orphan sweep by pid+exe, port find-or-fail). Never leak WINWORD/EXCEL.
- IPC: pipes for simple parent-child req/resp; Unix socket / Win named pipe for concurrent multi-window JSON-RPC. Cleanup stale socket on start (connect-test then unlink).
- MCP edge: internal live rpc core, MCP gateway outward. Cache `tools/list`, namespace `{server}.{tool}`, async only, kill sidecars on exit, validate schema before send, check `isError`.

## Digestion + porting protocol (ground-up, reuse don't rewrite)
1. Inventory source repo: license, runtime, transport, tool surface, security model. Record commit hash.
2. Extract protocol: opencode `SessionProcessor` loop / office `office-rpc/1` / MCP stdio — copy interface, not implementation.
3. Thin adapter: map source ops -> our 6 primitive ops (`read/write/format/struct/export/undo`). No forked business logic.
4. Security pass (below) + tests (fixtures + FakeSidecar fault injection) + docs (source + delta).
5. First sources: opencode loops, then office MCPs (OfficeMCP, mcp-office, ai-office-mcp 358 tools, word-mcp-server 108, dcc-mcp-office rpc+sidecar).

## Port 1: opencode loops (researched)
- Source: `session/processor.ts` `SessionProcessor.create()` `while(true)` per-step LLM stream; `session/prompt.ts` loop; `session/message-v2.ts` parts (text/reasoning/tool/file/agent/compaction/snapshot/subtask); `session/compaction.ts` overflow->turn split->budget prune (tool outputs 2000 chars, protect skill)->marker->auto-trigger; `session/retry.ts` exp backoff + RetryPart; agents `build/plan/general/explore` + hidden `compaction/title/summary` with permission merge.
- Port: event map `tool-input-start(pending)->tool-call(running + doom-loop check last-3 same tool+args -> ask)->tool-result(completed)/tool-error(error, loop continues)`; `start-step` snapshot.track() pre-stream, `finish-step` patch+tokens+compaction check; permission/question rejected -> blocked; stream abort + auth/context-overflow paths; token/cost tally per assistant message.
- Doom-loop, snapshot patch + revert, compaction markers, permission gates = required for live multi-file runs.

## Port 2: office MCPs (next)
- Order: dcc-mcp-office (rpc/STA/OpenXML/Graph) -> ai-office-mcp (152 Excel COM + 206 Word live + PPT skill, snapshot undo, track-changes) -> mcp-office (65/46/50 governed) -> word-mcp-server (108, visible undo-safe, .bak, chart isolation) -> OfficeMCP generic COM -> gawirable 47 file-level LibreOffice fallback.
- Map all to 6 ops; keep native undo (Word Ctrl+Z, Excel snapshot, PPT stack); macros disabled; path traversal block; Zod/JSON-schema `additionalProperties:false`.

## Per-tool prompt injection (browsed patterns, embed in every tool)
- Tool descriptions are model-visible input AND security control. Precise bounds: what it does + does NOT do + when to use. No dynamic unsanitized content in descriptions.
- Schema: `additionalProperties:false`, maxLength, numeric ids, hard caps on bulk (refuse not truncate), no raw SQL/shell/arbitrary paths. Separate read vs write; reads on read-only credential enforced by DB/OS, not prompt.
- Results untrusted: fence `<user_content>` + escape delimiters, `content_type` + `_meta.injection_flag`, return minimum (summary not body). Tool responses -> LLM as-is is the hole; sanitize + flag + gate next sensitive call on human confirm.
- Poisoning/rug-pull: pin hash(name+description+schema) at approval, diff every `tools/list`, quarantine drift, allowlist exposable tools only, human consent shows real args (model can't suppress client gate). Prefer signed manifests; wire poisoned-tool fixture into CI.
- Deletes soft by default; hard delete separate gated tool with preview token. Irreversible = host approval + server token.
- Log every call with redacted args; scoped session-limited credential so bypass blast radius is bounded. Never put secrets in prompt/tool results/history.
