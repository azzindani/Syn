# PRD — POC (Windows + MS Office, Single-User Single-Device)

Date: 2026-09-10 | Status: draft | Owner: the agent

## 1. Vision
This is a small movable widget: one desktop brain orchestrating many open software hands in one live session, any model via OpenRouter — mimicking how humans really work (multi-file, copy-paste, interrupt).

## 2. Problem
- Vendor sidebars are standalone per app with separate history and locked models.
- MCP file tools are headless-blind: human sees final output only, no watch/interrupt.
- Computer-use pixels work but cost $10/M in / $50/M out + slow screenshot loops.

## 3. User
Single user, single Windows device, Office installed (like Claude/ChatGPT Desktop shape). No org/tenant features in POC.

## 4. POC Goals
- Widget chat + background terminal jobs, always-on-top draggable.
- Live attach to open Word/Excel/PPT via COM (`GetActiveObject`, `Visible=true`); headless file mode via `Visible=false`.
- Multi-open registry (`app:type:pid:docId`), typed transfers with provenance (Excel range -> Word table -> PPT chart).
- Watchable run: broadcast event stream to widget, per-file undo scope, pause/resume/take-over.
- Model router: OpenRouter picker over four slots (small = skim / standard = routine / coding = code / reasoning = deep work and vision fallback only).

## 5. Non-Goals (POC)
No Mac/Linux builds (CI later), no Adobe/3D/CAD hands, no Marketplace listing, no multi-user, no cloud relay (localhost/named pipe only), no pixel-vision control.

## 6. Architecture
- Tauri v2 Rust widget (Preact/Svelte) ~8MB; tokio job queue with allowlist + kill; global hotkey + clipboard watcher.
- C# STA sidecar `office-host.exe` (office-rpc/1 JSON-RPC over named pipe): attach, busy-retry, modal detection, soft timeouts, snapshot undo, `.bak` before save, macros disabled.
- 6 primitive ops only: `read/write/format/struct/export/undo` (`struct` verbs cover trackChange, table, slide, pivot...). Tools expose only open handles (~12 max) to prevent context rot; per-tool embedded guidelines (what + NOT + when + 1 good/bad example).
- Internal live rpc core; MCP gateway outward for Claude/OpenCode compat. Cache `tools/list`, namespace `{server}.{tool}`.
- Port order: opencode loops (processor while-loop, parts, compaction/prune 2000 chars, retry backoff, doom-loop last-3 ask, snapshot patch/revert, permission gates) -> office MCPs (dcc rpc, ai-office 358, mcp-office governed, word-mcp 108 visible-undo).

## 7. Live Event Schema (relay rooms `session:{id}` + `file:{handle}`)
`step.start/step.live{preview,undo}/step.done/xfer{from,to,rows}/paused` — widget + all panes render same feed.

## 8. Security (per-tool injection aware)
Closed schemas (`additionalProperties:false`, caps, refuse-not-truncate), read/write split with least-privilege creds, soft-delete default, destructive = preview token + host approval, results fenced untrusted + `injection_flag`, metadata pin/diff + allowlist + human consent on sensitive, redacted logs, no secrets in prompt/history.

## 9. Milestones + Acceptance
- M1: widget + job queue + sidecar hello (open blank Word via terminal, write line, live visible). Accept: cold start <2s, no orphan WINWORD on kill.
- M2: registry + 6 ops on 1 Word + 1 Excel (read/write/undo). Accept: wrong-handle calls rejected, undo restores.
- M3: demo run — 2 Excels + 1 Word open, transfer ranges live with flashes + provenance, human pauses once, resumes, deck/table correct. Accept: full event log + per-file undo works + cost logged per model.
- Exit POC when M3 demo passes 3/3 runs; then plan Office.js panes + CI + next hands.

## 10. Risks / Open
COM single-instance quirks, modal dialogs blocking STA, file locks/co-author conflicts, WebView localhost limits (deferred via named pipe), vision-fallback cost caps.
