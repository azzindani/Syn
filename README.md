# Syn — one brain, many hands

Single-user desktop harness: a small movable widget orchestrates many open
software hands (Office first) in one live session, any model via OpenRouter.

## Layout
- `protocol/` — rpc catalog + default-deny security policy (JSON, language-neutral).
- `relay/` — Python POC of the relay core (stdlib only).
- `core/` — production Rust core (`harness-core`, zero deps):
  - `protocol/bus/ops/security` — op vocabulary, relay, dispatch, policy.
  - `sessions` — multi-session hub (M5): independent runners, list/drop.
  - `memory` — session facts + cross-app transfer log + compaction (M2/M3).
  - `guard` — runtime allowlist + kill switch, enforced in `Runner::pump`.
  - `provider` — OpenRouter bodies, SSE streaming parse, Picker + budget.
  - `mcpgate` — MCP gateway: 6 ops as MCP tools over stdio.
  - `router/queue/runner/paths/stream/snapshots/acp/vfs/ooxml` — as before.
  - bins: `harness` (REPL), `mcpgate` (stdio bridge).
- `widget/index.html` — chat + live feed + model picker + budget + kill
  (Tauri shell in `widget/src-tauri`; UI runs standalone in demo mode).
- `sidecar-csharp/Host/` — full STA COM sidecar (Word/Excel/PowerPoint,
  named-pipe office-rpc/1, timeout-guarded calls, .bak snapshots).
  Needs Windows + .NET 8 + Office to compile/run.
- `office-pane/` — Office.js task pane (Mac/Web hand), sideload to verify.
- `docs/` — harness-00→08, DIGEST-00→04, PRD, ideas, runbook-windows.
- `.tmp/repos/` — 9 cloned sources this was ported from.

## Port map (digested in docs/DIGEST-*)
- opencode loops -> bus events, part states, compaction budgets, retry,
  doom-loop gate, snapshot patch/revert, permission ask.
- dcc-mcp-office -> office-rpc/1 methods, two-layer default-deny, job phases,
  single STA write queue per sidecar (= per-file mutex).
- ai-office-mcp -> dual COM/openpyxl backends (= live/headless duality),
  SnapshotUndo stack. mcp-office -> env allowlist gates. word-mcp-server ->
  status enum, auto-.bak, Manager API. office-agents -> pane+bridge BYOK shape.
  gawirable -> LibreOffice fallback/export path.

## Run here (Linux POC)
```
python3 -m unittest discover -s tests   # 23 green
cd core && cargo test                      # 116 green
cargo run --example demo && ./target/release/harness < ../tests/e2e_script.txt
./target/release/harness < ../tests/e2e_safety.txt   # allow/kill/journal/replay/sessions
printf '{"tool":"list"}\n' | cargo run -q --bin mcpgate
```
Windows-only (see docs/runbook-windows.md): `dotnet build` the sidecar,
live Word/Excel typing, Tauri bundle, real OpenRouter key.
