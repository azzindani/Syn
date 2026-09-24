# DIGEST-02 — Office Ports (in 08 order)

## 1. dcc-mcp-office (rpc + sidecar) — PORT FIRST
- Wire: `office-rpc/1` (catalog v1.2): `office.host.handshake/ping`, `office.job.get/cancel`, `office.command.execute` (`crates/office-protocol/office-rpc.catalog.json` + JSON schemas).
- Host: C# `Office.Automation.Host` (Program, OfficePipeServer named pipe, CommandRouter, CommandPolicy, CapabilityCatalog, InMemoryJobTracker, ParentProcessMonitor, TemplateRegistry, JsonSchemaValidator) + `Office.Automation.Runtime` (STA dispatcher, IOleMessageFilter busy retry, modal detection) + `Office.Automation.Com` (per-app backends) + `Office.Automation.OpenXml` (batch worker, compiler-not-renderer).
- Jobs (`office-jobs`): Queued->Planning->WaitingForApproval->Running->Validating->Publishing->Succeeded/PartiallySucceeded/Failed/Cancelled; batch = job, never block MCP req; per-item outcomes; OpenXML parallel across files, each COM sidecar single STA write queue, same-doc writes mutually exclusive — ADOPT as the agent per-file mutex.
- Security (`office-security`, `#![forbid(unsafe_code)]`, two-layer: Rust gateway pre-check + C# COM-boundary re-check, AutomationSecurityForceDisable on untrusted, XLM4.0 separate detect): default-deny macros/VBA-run/external-links/OLE/protected-view-bypass/arbitrary-ExecuteMso; print/send-mail/meetings/publish = confirm; overwrite = checkpoint_and_confirm; workspace_only; ExecuteMso allowlist (empty=deny all). ADOPT table into the policy here.
- Rust crates: protocol/client/ir/jobs/graph/security/testkit(FakeSidecar fault injection)/tools/mcp-server. Tests + proposals/adr present.

## 2. ai-office-mcp (COM maps) — PORT SECOND
- Excel (`excel/excel_mcp/`): dual backend `core/excel_com.py` (live GetActiveObject-style) + `core/openpyxl_backend.py` (headless) — PROOF of live/headless duality; 22 tool modules (calc/chart/comment/compat/diff/file/format/image/named_range/pivot/range/screenshot/sheet/slicer/table/undo/vba/window); `SnapshotUndo` stack: `undo_last/list_undo_snapshots/clear_undo_history` per workbook — ADOPT per-handle snapshot#.
- Word-live (`word-mcp-live/`, ykarapazar MIT): open-doc live edit, native track-changes, threaded comments, single-step Ctrl+Z, layout diagnostics, `tool_registry.py` + `defaults.py` (MCP_AUTHOR signature).
- PPT (`ppt-master-main`, hugohe3): Skill workflow PDF/DOCX/URL -> native DrawingML (not images), 22 examples/309pp cases.
- Map all verbs -> the agent 6 ops; keep COM verbs Windows-gated with openpyxl fallback (matches 152/67 split).

## 3. mcp-office dosev-ai (governed maps) — PORT THIRD
- `excelmcp`(65)/`pptmcp`(46)/`wordmcp`(50) + `shared/`; env `*_ALLOWLIST_ROOTS` + `*_ENABLE_WRITE`; COM tools need Office, file tools don't; smithery.yaml + Dockerfile + SECURITY.md + ROADMAP.md. ADOPT env-gate pattern + governed deterministic calls.

## 4. word-mcp-server HelloWorld-Open (Word verbs) — PORT FOURTH
- Node winax COM, watchdog parent(30s restart)/child(McpServer), chart-data fork+15s timeout; 81 tools/13 modules (lifecycle/content/format/tables/charts/images/structure/clipboard/Manager API/semantic nav/variables); `word_get_status` NO_WORD/NO_DOC/DOC_ACTIVE/DIALOG — ADOPT status enum for live attach; auto `.bak`, macros disabled, Zod validation, rate-limit, audit log. Clients: Claude/Cursor/OpenCode/Codex.

## 5. office-agents RaulAM7 (thin-pane pattern) — PORT FIFTH
- `packages/{sdk,core,bridge,excel,word,powerpoint}` + `office-bridge/`; BYOK OpenAI/Anthropic/Google/Azure/OpenRouter/Groq/xAI/Cerebras/Mistral + OAuth + custom endpoints; Office.js wrappers + system prompts per app; local HTTPS/WS bridge + CLI inspects live runtime + VFS + sandboxed shell. ADOPT pane+bridge shape; the agent differs: chat moves to desktop widget, panes become hands.

## 6. gawirable (fallback) + ideas-only
- gawirable 47 tools, `architecture.md` authority, LibreOffice headless export (mammoth for docx->html), `.COM`-shim warning — ADOPT as no-Office fallback + export path.
- hinora (WPS/Outlook/WhatsApp CDP/.exe builds), OfficeMCP (`RunPython(Officer.*)` god-tool): NO license found -> ideas only, no code.
