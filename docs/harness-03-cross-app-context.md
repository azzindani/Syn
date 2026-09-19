# Harness 03 — Cross-App Context Transfer

## Goal (from thread)
Write research presentation by reading paper then writing to PowerPoint. Read Word docs and prepare PPT from them. Context transfer between softwares, one interface session.

## How, headless, no pixels
1. Reader: `word.read` / PDF fetch / Graph read -> chunk -> summary JSON `{claim, evidence, figures, outline}`.
2. Store: `session_abc: {source_doc, outline, facts[]}` on desktop brain.
3. Writer: `ppt.createSlides(layout, title, bullets, notes)` via PowerPoint.js (small) or file-gen (large) using slide master so output stays editable with native charts/tables.
4. Word->deck mapping: headings -> slide outline -> 10 slides + speaker notes. Never dump 80 pages raw per prompt.

## Why external brain required
- Word add-in cannot open Excel/PPT directly (sandbox wall). Only external orchestrator can hold Word+Excel+PPT+PDF+web in one session and route between them.
- Claude M365 precedent: one conversation Outlook->Word->Excel->PPT, change propagates to open files. Desktop-alone (Graph) = file-level new files only, no live open-deck injection.

## Connectors-per-software (from thread)
- One connector per app, same shape `manifest + tools[] + auth + session_id`. Build only what flow needs: Word+P present first.
- Browse = same shape: (1) Tavily/Brave search + fetch->markdown (cheapest, 95%), (2) Chrome/Edge extension exposing tab DOM/selection to session, (3) built-in browser/WebMCP only for login-walled sites.

## Limits
- PPT.js weak on design fidelity. Verify by opening, not screenshots in v0.
