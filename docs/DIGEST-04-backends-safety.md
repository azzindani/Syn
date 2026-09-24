# DIGEST-04 — Backends, Context + Safety Patterns

## Dual backends everywhere (ai-office-mcp, MIT README) — ADOPT as live/headless duality
- Excel: `excel_com.py` (live GetActiveObject-style, pivots/slicers/VBA/charts/screenshots/window mgmt) vs `openpyxl_backend.py` (headless, 67 tools cross-platform). Same tool names, capability degrades by platform.
- `SnapshotUndo`: per-workbook stack, disk-persisted index with orphan pruning (survives restarts), `push(wb)->snap_id`; `undo_last/list/clear`. UPGRADE over the agent in-memory snapshots: persist index + prune orphans.
- Word-live: open-doc editing, native track-changes, threaded comments, per-action Ctrl+Z, layout diagnostics, `MCP_AUTHOR` attribution.
- PPT skill: PDF/DOCX/URL -> native DrawingML (never rasterized slides), example-driven (22 projects/309pp).

## mcp-office governed layer (dosev-ai, MIT) — ADOPT gates
- `mcpshared`: Artifact Context Packet v1 — 3-level responses (Index always -> Focused -> Deep+annotations) so callers request only needed context (= compaction-by-construction). Formula-injection sanitiser on annotations (Excel `=+-@` prefix strip — adopt for all cell writes).
- Env gates `*_ALLOWLIST_ROOTS` + `*_ENABLE_WRITE`; COM tools need Office, file tools don't; Dockerfile + smithery.yaml + SECURITY.md.

## dcc platform proposal (MIT, Chinese doc) — CONFIRMS the vision
- Rust server + per-app COM sidecars + OpenXML worker + Graph + Office.js + UIA/Computer-Use fallback ladder; task-level (not API-dump) tools; operates open docs/selections/cells; native render/calc/export; DCC outputs (Maya/Houdini/Blender/Unreal/Photoshop) auto-composed into review PPT/reports.
- Pipe: explicit-DACL `CreateNamedPipe`, InOut — adopt DACL note for sidecar hardening.

## gawirable (MIT) + unlicensed (ideas only)
- gawirable 47 tools; `architecture.md` authority; LibreOffice headless export; mammoth docx->html; Windows `.COM`-shim hang trap (prefer .exe, set explicit soffice path). Adopt as no-Office fallback.
- hinora (WPS/Outlook/WhatsApp-CDP/exe builds), OfficeMCP (`RunPython(Officer.*)` god-tool): NO license -> shapes only.

## Desktop Agent deltas queued from this layer
1. hello/snapshot handshake (documentId+tools+host) into attach.
2. VFS verbs + CLI strip-images hygiene + unsafe-eval dev-only.
3. Session paths idle|streaming|editing + watchdogs + circuit breaker.
4. Stream start/block/end live-writing UX for struct.*.
5. Disk-persisted snapshot index + orphan prune.
6. ACP 3-level responses + formula sanitiser on cell writes.
7. Screenshot tools as the sanctioned vision-fallback path (Astra-routed).
8. Follow-mode selection tracking into live feed.

## Implementation status (loop D3, verified)
All 8 deltas now live in `core` (zero-dep, clippy -D clean, 30 tests):
1. `bus.attach_with_snapshot` + `follow/selection` (hello: document_id/tools/host; follow events in feed).
2. `paths.rs`: SessionPath idle|streaming|editing + breaker (trips at 3, human reset).
3. `stream.rs`: stream_start/block/end with 20k block cap + counts.
4. `snapshots.rs`: disk index + orphan prune (upgrade over in-memory bus snapshots).
5. `acp.rs`: Index/Focused/Deep gating + `sanitise_formula` applied on every excel write.
6. `vfs.rs`: root-confined list/read/write/delete, traversal rejected.
7. `Export{format}`: summary|preview, preview ships VisionFallback routing note.
8. Follow-mode selection tracked per handle into event stream.

## File-output extension (loop F, verified)
- `core/src/ooxml.rs`: stored-ZIP writer (own CRC32) + minimal xlsx (numeric vs inlineStr typing so formulas survive) + docx (paras + native tables, marker paras skipped). Zero deps.
- `export xlsx|docx <path>` writes real files (parents auto-created); independent `zipfile` parse proves valid archives with content. pptx honestly deferred.
