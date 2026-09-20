# 01 — MS Office Connector

## Platform (verified)
- Modern: Office Add-ins (Office.js), unified M365 manifest (JSON) or add-in-only manifest (XML). Task pane = web app (React+TS). Scaffold: `npm install -g yo generator-office` -> `yo office`.
- Refs: learn.microsoft.com/en-us/office/dev/add-ins/quickstarts/word-quickstart-yo ; github.com/OfficeDev/Office-Add-in-samples
- Legacy Windows-only: VSTO/COM sidecar. Keep only for VBA `Application.Run` + deep live control.

## Live agent ops
- Word: `Word.run()` insert/replace/format, tables/images/HTML, content controls, comments, tracked changes.
- Excel: ranges/tables/formulas/charts/custom functions, multi-tab read.
- PowerPoint: weakest — text/bullets/tables/native charts OK; big/pixel-perfect decks via Graph/file-gen then open.
- Outlook: separate mail add-in type.

## Thin connector pattern (the fix for standalone problem)
- Add-in hosts NO chat. It joins `session_id` and runs 3-5 tools: Word `read/insert/trackedChange`, Excel `readRange/writeRange/explain`, PPT `createSlides/applyLayout`.
- Relay (wss) not localhost: Office WebView blocks raw localhost in prod, AppSource rejects it.
- One desktop session fans out to Word+Excel+PPT simultaneously.

## VBA / Macro bridge (Excel)
- Office.js CANNOT exec VBA (sandbox). Indirect store-safe: add-in writes inputs to named ranges -> existing VBA runs (button/Open event) -> writes outputs -> add-in reads back.
- Modern: Office Scripts (TS) callable via Graph/Power Automate REST from the agent.
- Windows power: VSTO/COM sidecar CAN `Application.Run("Module1.Macro")`.

## Vendors today (verified)
- OpenAI: ChatGPT for Excel+PowerPoint, Marketplace, Business/Enterprise/Edu. Separate history. Codex-in-desktop CAN control open Excel via add-in.
- Anthropic: Claude for Excel/PowerPoint/Word GA + Outlook beta, sidebar carries context across apps. Desktop M365 connector = separate Graph path.
- Takeaway: standalone works, but no shared desktop session + no external models — the gap the agent fills.
