# 06 — Prior Art: Did Someone Build This Already? (Deep Research 2026-09-10)

## Short verdict
No single product = your exact combo (custom desktop one-session brain + thin Office.js hands + COM live/headless + OpenRouter/DeepSeek + cross-app Word->PPT + Adobe/3D/CAD).
But 80% exists in pieces. Closest collisions below. Your gap = unified external-model session outside vendor clouds + live COM + cross-software.

## 1. Vendors already do cross-app (your core flow)
- Microsoft 365 Copilot Agent Mode (Word/Excel, PPT coming) + Office Agent in Copilot Chat: pull Excel -> Word report -> PPT summary without switching apps. Multi-agent orchestration via Graph + Entra. GA Apr 2026 for Word/Excel/PPT.
- Microsoft TechCommunity: they explicitly built an `agentic harness` with isolated sandbox, brokered WebSocket, headless Word/Excel/PowerPoint building real files (not XML stitch), reusing Agent Mode skills. Enterprise, Anthropic-models-gated, no external providers.
- Claude for M365: one conversation across Word/Excel/PowerPoint/Outlook add-ins. File-level via Desktop Graph connector otherwise.
- ChatGPT for Excel/PowerPoint: standalone sidebar, separate history; Codex desktop can drive open Excel via add-in.
Implication: paper->PPT context transfer is proven, but locked to M365/Anthropic/OpenAI clouds.

## 2. Open source almost exactly your Office.js idea
- `office-agents` monorepo (RaulAM7/hewliyang/sinag forks): Word+Excel+PowerPoint Add-ins with integrated AI chat panels, BYOK OpenAI/Anthropic/Google/Azure/OpenRouter/Groq/xAI + Ollama/vLLM, Office.js wrappers, headless SDK, VFS, sandboxed shell, local HTTPS/WebSocket RPC bridge + CLI. This is the closest public prior art to thin-connector + external models. Difference: chat lives per-app, not one external desktop session orchestrating all apps; no COM live attach, no Adobe/3D/CAD.
- Univer (univer.ai): `The Office Harness for AI Agents` — but own Sheets/Docs/Slides runtime (.univer file), not native MS Office. Different bet.

## 3. Open source already does COM/MCP live control (your terminal/COM idea)
- OfficeMCP/OfficeMCP: COM Word/Excel/PPT/Access/Visio/Project + WPS, `Officer.Excel/Word`, `RunPython` tool. Windows-only.
- hinora/office-mcp: WPS + Outlook + web tools via win32com, .exe releases, MCP stdio.
- dosev-ai/mcp-office: excelmcp 65 + pptmcp 46 + wordmcp 50 tools, local-first, allowlist roots, COM styling/PDF/tracked-changes, Claude Desktop/VS Code client. Explicitly not a Copilot replacement.
- lingfan36/ai-office-mcp: 358 tools measured (Excel 152 COM + Word-live 206 + PPT skill), live open-doc edit, native track changes, snapshot undo, local-only except LLM, model-free (Claude/GPT/Gemini/Kimi).
- word-mcp-server (HelloWorld-Open/Han-ops): 80-108 tools, visible WINWORD, Ctrl+Z native, macros disabled, MCP stdio for Claude/Cursor/OpenCode/Codex.
- gawirable/office-mcp: 47 tools file-level docx/xlsx/pptx + LibreOffice headless export, cross-platform.
- dcc-mcp-office: office-rpc/1 + C# STA COM sidecar + OpenXML worker + Graph connector + skill packs. PowerPoint/Word/Excel adapters. Closest to your C# sidecar vision.
Implication: COM-live + MCP + Claude Desktop-as-brain already commoditized. Your twist must be custom desktop session (not Claude Desktop) + Office.js thin add-ins (Mac/Web) + COM (Win) under one relay + OpenRouter picker.

## 4. Desktop multi-model agents (your orchestrator idea)
- OpenDesktop: 50+ providers, OS control, self-rewriting agent.
- open-cowork (open Claude Cowork): Windows/macOS GUI, Claude/OpenAI/Gemini/DeepSeek via OpenRouter, WSL2/Lima sandbox, PPTX/DOCX/XLSX skills, MCP browsers, GUI computer-use.
- Cortex Workspace: desktop agent, all models one plan, native Office editor bypassing heavy apps, terminal skills.
- OpenRouter: one endpoint 500+ models, MCP server, cookbook for Claude Desktop Gateway + Codex desktop `config.toml` provider.
Implication: external-model desktop brain exists. None bundles native Office.js + COM live + Adobe/Unreal/CAD matrix as first-class hands.

## What is still open (build this)
1. One external session ID spanning Office.js (Mac/Web) + COM live (Win) + browser + Adobe UXP + Blender/Unreal/AutoCAD, with OpenRouter picker and shared history — nobody ships this bundle.
2. Relay pairing (6-digit) + allowlisted background shell + GetActiveObject live attach as product UX, not GitHub README.
3. API-first router that never pays Astra pixels unless Office.js/COM/Graph provably can't reach the control.
