# 00 — Overview: One Brain, Many Hands

Date: 2026-09-10 / swept full thread
Status: idea bucket

## Where this started
- Asked for big anti-mainstream projects (see `ideas-00-big-bets.md` + `ideas-01-coding-projects.md`).
- Narrowed to: MS Office family plugin for agents -> can we control Word/Excel/PPT from one desktop session with external models (OpenRouter/DeepSeek), not standalone sidebars?
- Answer became this project.

## Problem (from thread)
- OpenAI: `ChatGPT desktop app` (new: Chat+Work+Codex, old renamed `ChatGPT Classic`). Anthropic: `Claude Desktop`. Both support in-app model change, but locked to first-party models by default.
- Office plugins are standalone: ChatGPT for Excel/PowerPoint (Marketplace, Excel+PPT only, separate history; Codex desktop CAN drive open Excel via add-in, Excel-only). Claude for Word/Excel/PowerPoint (GA paid) + Outlook (beta), one conversation across Office apps. Neither runs inside the desktop AI app.
- Claude Desktop alone = Graph file-level (OneDrive/SharePoint/Outlook/Teams read, write with scopes), not live open-doc UI control. Live tracked-changes / cell / slide-master work needs add-in hands.
- Built-in first-party wall: Office.js iframe can only touch its own doc + https out. No cross-app, no OS, no localhost in prod. Hence fragmentation.
- GPT-6 Astra computer-use is best (`gpt-6-astra`, 72.6% OSWorld 2.0, ~47% faster than Sol) but heavy: screenshot loops, $10/M in / $50/M out, long-context surcharge, per-call fee.

## Vision
External desktop orchestrator + thin connectors, one `session_id`:
- Brain outside platform. Hands inside each platform.
- Hands: Word/Excel/PPT add-ins, browser extension, Unreal Remote Control, Blender bpy, Photoshop UXP, AutoCAD accoreconsole.
- Pair via 6-digit code (Live Share style). Relay `wss://relay/session_id` to dodge localhost/AppSource blocks.

## Rules
- API-first, vision-fallback (95/5).
- External providers first-class (OpenRouter/DeepSeek), shared history everywhere.
- No standalone chat per app. One conversational box on desktop + background terminal jobs.
- v0 headless, no screenshots: files + COM + REST. Vision only later as fallback.

## Bucket index
- 01 office connector (Office.js + COM live/headless + VBA)
- 02 desktop orchestrator (chat + background terminal, blank launches, live attach)
- 03 cross-app context transfer (paper->PPT, Word->deck)
- 04 adobe / 3D / CAD matrix (verified)
- 05 models, cost, routing + browsing
- ideas-00 / ideas-01 origins
