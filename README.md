# Syn — one brain, many hands

Single-user desktop orchestrator: a small movable widget drives many open
software hands (Office first) in one live session, any model via OpenRouter.

## Layout
- `protocol/` — rpc catalog + default-deny security policy (JSON, language-neutral).
- `relay/` — Python POC of the relay core (stdlib only).
- `core/` — production Rust core (`core`, zero deps):
  - `protocol/bus/ops/security` — op vocabulary, relay, dispatch, policy.
  - `sessions` — multi-session hub (M5): independent runners, list/drop.
  - `memory` — session facts + cross-app transfer log + compaction (M2/M3).
  - `guard` — runtime allowlist + kill switch, enforced in `Runner::pump`.
  - `provider` — OpenRouter bodies, SSE streaming parse, Picker + budget.
  - `mcpgate` — MCP gateway: 6 ops as MCP tools over stdio.
  - `router/queue/runner/paths/stream/snapshots/acp/vfs/ooxml` — as before.
  - `hand` — `office-rpc/1` transport to a live sidecar (the brain's
    connection to the hands; generic over the stream, so testable anywhere).
  - bins: `cli` (REPL, incl. `hand`/`lread`/`lwrite`), `mcpgate` (stdio bridge).
- `widget/index.html` — chat + live feed + model picker + budget + kill
  (Tauri shell in `widget/src-tauri`; UI runs standalone in demo mode).
- `sidecar-csharp/Host/` — full STA COM sidecar (Word/Excel/PowerPoint,
  named-pipe office-rpc/1, timeout-guarded calls, .bak snapshots).
  Needs Windows + .NET 8 + Office to compile/run.
- `office-pane/` — Office.js task pane (Mac/Web hand), sideload to verify.
- `scripts/` — Windows bring-up: `new-testbed-docs.ps1` (fixtures via COM),
  `live-excel-smoke.ps1` (M1/M2 acceptance vs real Excel), `pipe-client.ps1`.
- `testbed/` — gitignored run area for live Office tests (docs/out/logs).
- `.env.example` — provider key, endpoint, and the four model slots
  (`SYN_MODEL_LUNA|TERRA|SOL|ASTRA`); copy to `.env`, which is gitignored.
- `docs/` — 00→08, DIGEST-00→04, PRD, ideas, runbook-windows.
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
cd core && cargo test                      # 123 green
cargo run --example demo && ./target/release/cli < ../tests/e2e_script.txt
./target/release/cli < ../tests/e2e_safety.txt   # allow/kill/journal/replay/sessions
printf '{"tool":"list"}\n' | cargo run -q --bin mcpgate
```
## Run on Windows (live Office)
```
dotnet build sidecar-csharp\Host\Host.csproj -c Release
powershell -File scripts
ew-testbed-docs.ps1     # fixtures (close Office first)
powershell -File scripts\live-excel-smoke.ps1     # attach + read/write live Excel
```
Verified on Windows 11 + Microsoft 365 + .NET 8: the sidecar attaches to an
open workbook, reads and writes it live, fails closed on bad handles, and
detaches without closing the user's Excel — driven both by the PowerShell
smoke test and by `cli.exe` itself over the pipe (`hand` / `lread` / `lwrite`). Word/PowerPoint dispatch, export,
the Tauri bundle and a Rust-side pipe client are still unproven — status and
the COM lessons behind the fixes are in `docs/runbook-windows.md`.

Copy `.env.example` to `.env` and add a key to enable `send`.
