# DIGEST-00 — Source Repo Inventory (.tmp/repos (.tmp moved under product dir), shallow depth-1, 2026-09-10)

## Table
| # | Dir | Upstream | Size | License | Runtime | Transport | Tool surface | Port priority |
|---|-----|----------|------|---------|---------|-----------|--------------|---------------|
| 1 | sst-opencode | sst/opencode (upstream; thread refs anomalyco fork) | 224M | MIT | Bun+TS, Effect, SQLite/Drizzle | — (loop core) | ~20 tools, each `id + description(.txt) + parameters(Schema) + execute` | P0 loops |
| 2 | dcc-mcp-office | dcc-mcp/dcc-mcp-office | 32M | MIT | Rust crates + C# STA sidecar + OpenXML worker | named pipe office-rpc/1 + Graph | task-level registry, jobs w/ approval | P1 rpc/sidecar |
| 3 | ai-office-mcp | lingfan36/ai-office-mcp | 26M | MIT (README) | Python COM + PPT skill workflow | MCP stdio | 152 Excel + 206 Word-live + PPT DrawingML skill (358 measured) | P1 COM maps |
| 4 | word-mcp-server | HelloWorld-Open/word-mcp-server | 19M | MIT | Node/TS winax COM, watchdog parent+child | MCP stdio | 80-108 Word tools, TOOLS.md, Manager API | P2 Word verbs |
| 5 | office-agents | RaulAM7/office-agents | 11M | MIT (pkg) | TS monorepo sdk/core/bridge + excel/word/ppt add-ins | Office.js + local HTTPS/WS bridge | BYOK providers, VFS, sandboxed shell | P2 thin-pane pattern |
| 6 | mcp-office | dosev-ai/mcp-office | 5.6M | MIT | Python FastMCP excelmcp/pptmcp/wordmcp + shared | MCP stdio | 65 + 46 + 50 governed tools, allowlist roots | P2 governed maps |
| 7 | gawirable-office-mcp | gawirable/office-mcp | 1.4M | MIT | Python FastMCP + LibreOffice headless | MCP stdio | 47 file-level tools, arch in architecture.md | P3 fallback |
| 8 | hinora-office-mcp | hinora/office-mcp | 1.2M | none found | Python win32com + .exe builds | MCP stdio | WPS+Outlook+WhatsApp(CDP)+web | ideas only |
| 9 | officemcp | OfficeMCP/OfficeMCP | 424K | none found | Python FastMCP COM | MCP stdio | `RunPython(Officer.Word/Excel/...)` god-tool | ideas only |

## Rules applied
- MIT (files or README/pkg claim): may port code with attribution + hash recorded.
- No license found (hinora, officemcp): digest ideas/API shapes only, NO code copy.
- Clones live in `/workspace/data/syn/.tmp/repos` (NOT shared data/). Docs live in `docs/`.
