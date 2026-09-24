# DIGEST-03 — Bridge + Session Patterns (office-agents + word-mcp-server)

## office-agents bridge v1 (RaulAM7, MIT) — ADOPT wire shape
- Transport: local HTTPS + WSS `wss://localhost:4017/ws`, 30s req timeout, 200-event cap (`packages/bridge/src/protocol.ts`).
- Handshake: pane sends `hello{role:office-addin, protocolVersion, snapshot{documentId, instanceId, app, tools[], host{platform, officeVersion, href, title}}}`; server replies `welcome{protocolVersion, serverTime}`.
- Methods: `ping/get_session_snapshot/refresh_session/execute_tool/execute_unsafe_office_js/vfs_list/read/write/delete`.
- Events: `{type:event, event, ts, payload}` + stored ring buffer; CLI `list/inspect/metadata/tool/exec/events`, strips image base64 from printed JSON (context hygiene).
- VFS: text/base64 read-write-delete with byteLength — file staging without leaving the workspace.
- `execute_unsafe_office_js` escape hatch: full taskpane eval in dev, sandboxed app-tool route in prod. the agent: dev-only, never prod.
- Port deltas for the agent: panes are hands not chats (move AgentRuntime+chat to desktop widget); snapshot.hello maps to our attach{handle, kind, tools}; VFS verbs map to read/write/export.

## AppAdapter pattern (per-app thin layer) — ADOPT
- Interface: `tools[] + buildSystemPrompt(skills) + getDocumentId() + getDocumentMetadata() + onToolResult() + metadataTag + Link/ToolExtras + emptyState`.
- Core ChatInterface stays generic; Excel/PowerPoint/Word adapters inject tools/prompts/metadata + follow-mode (selection tracking) + navigation on results.
- Per-app escape hatches: excel `eval_officejs`, word/ppt `execute_office_js`; word `screenshot-document`, ppt `screenshot-slide`/`edit-slide-xml`/`edit-slide-chart` (JSZip OOXML), excel `set/get-cell-ranges`.
- Stack: React18+TS+Vite6+Tailwind, pi-ai/pi-agent-core LLM core, just-bash VFS shell, IndexedDB sessions (`OpenExcelDB_v3`), Biome, per-app Cloudflare releases.

## word-mcp-server session machinery (HelloWorld-Open, MIT) — ADOPT
- `SessionDirector`: watchdog 5s / hung 30s, circuit breaker, stream/edit locks, precondition DOC|NO_DOC, blocked sets (STREAM_BLOCKED during stream, EDIT_BLOCKED during edit), read-only tool bypass.
- `SessionPathMachine`: paths idle|streaming|editing + 10-min streaming watchdog — maps to the agent queue states + pause semantics.
- Stream pattern `stream_start(baseStyleProfile+template) -> stream_block(markdown, instant Word preview) -> stream_end(save[+pdf], counts+elapsed)` — the live-writing UX our struct verbs should mirror.
- Status enum NO_WORD/NO_DOC/DOC_ACTIVE/DIALOG — pane/COM presence + modal detection (adopt as attach health).
- Watchdog parent (30s restart) + chart-data fork (15s timeout) — adopt process supervision shape.
- TOOLS.md 81 tools/13 modules: lifecycle/content/format/tables/charts/images/structure/clipboard/Manager API/semantic nav/variables — verb source for struct.* mapping.
