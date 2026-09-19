# 07 — Live Running Orchestration (Human Eyes Spec)

Date: 2026-09-10
Status: spec v0 — answers MCP headless-blind problem

## Problem (from thread)
MCP file tools = headless only. Human sees final output, not the run. No trust, no interrupt, no co-work. People need live running orchestration they can watch.

## Principle
Same session, two modes, one event stream:
- Bulk headless for speed, visible live when human watches/corrects. Flip per file, mid-run.

## Modes
- Headless: `Visible=$false`, openpyxl/pptx/docx, LibreOffice convert, Graph file write. No window.
- Live: `GetActiveObject` + `Visible=$true`, Office.js selection highlight + taskpane status. Bot edits where human looks. Cell flash / para highlight / slide thumbnail on each step.

## Event stream (relay broadcasts to desktop + all panes)
```json
{"t":"step.start","session":"abc","handle":"excel:plan.xlsx:Sheet1","tool":"writeRange","args":"A1:D20","by":"agent"}
{"t":"step.live","handle":"excel:plan.xlsx:Sheet1","preview":"range-flash A1:D20","undo":"snapshot#41"}
{"t":"step.done","handle":"word:report.docx","op":"insertTrackedChange p12","review":"accept/reject"}
{"t":"xfer","from":"excel:plan.xlsx!A1:D20","to":"ppt:deck.pptx:Slide4:Table1","rows":20}
{"t":"paused","by":"human","at":"step 14"}
```
- Desktop chat + every open add-in render same feed. No silent steps.

## Multi-open routing (from thread)
- Registry: `app:type:pid:docId` per open file. Tools must address handle explicitly, never bare active.
- COM: ROT enumerate N Words/Excels by title/path. Office.js: one doc per pane, relay multiplexes N panes -> one session.
- Transfer = typed IR move (table/range/slide), with provenance `cell X <- doc Y range Z`. Not pixel copy-paste.

## Interrupt + undo
- Per-file undo scope: Word native Ctrl+Z chain, Excel snapshot+`excel_undo`, PPT step stack. Stop button halts queue, keeps files open, `.bak` before save.
- Human take-over: pause -> human edits live -> resume, agent re-reads handle before next step.

## Switch policy (API-first, Astra-last)
1. Office.js/COM/Graph if reachable. 2. Visible attach if user opened app. 3. Headless bulk otherwise. 4. Astra vision only for UI-only gaps. Never pixels for what API can do.

## MVP build
- Relay rooms: `session:{id}` global + `file:{handle}` per doc.
- Tools: `registry.list`, `read`, `write`, `preview`, `undo`, `pause/resume`, `xfer`.
- Demo: 2 Excels + 1 Word open, transfer ranges live with flashes, human pauses once, resumes, final deck shows provenance.
