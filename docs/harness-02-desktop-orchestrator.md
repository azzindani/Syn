# Harness 02 — Desktop Orchestrator (Chat + Background Terminal)

## v0: conversational box + background shell, no screenshots
- UI: Tauri+React (product) or Streamlit (days-prototype). One input, one thread, `session_id`.
- Backend: job queue `shell(cmd)` with allowlist, streaming stdout, kill switch. Must run as same user session for COM live.
- Time-saver: v0 is only chat->shell->file. Office.js connectors plug in later as cheaper tools on same session.

## Open software via terminal (no clicks)
- Win: `start winword` / `start excel` / `start powerpnt` (blank if no arg). `Start-Process winword.exe "doc.docx"`. Protocols `ms-word:` / `ms-excel:` open UI.
- Mac: `open -a "Microsoft Word"` / Excel / PowerPoint.
- Headless file mode (no python-docx needed, Office installed, Windows):
```powershell
$w = New-Object -ComObject Word.Application
$w.Visible = $false
$d = $w.Documents.Add()
$d.Content.Text = "Hello from harness"
$d.SaveAs("C:\tmp\out.docx")
$w.Quit()
```
- Same shape for `Excel.Application` (`Workbooks.Add`) and `PowerPoint.Application` (`Presentations.Add`).

## Background control that stays live in UI
- `New-Object` = hidden. `GetActiveObject` = live open app user watches:
```powershell
$w = [Runtime.InteropServices.Marshal]::GetActiveObject("Word.Application")
$w.ActiveDocument.Content.InsertAfter("added from harness")
$e = [Runtime.InteropServices.Marshal]::GetActiveObject("Excel.Application")
$e.ActiveSheet.Range("A1").Value2 = "live update"
```
- Limits: same user, app open, single instance, no Session-0 service. File locks / co-author conflicts possible.

## Vision = optional fallback only
- Terminal launch + screenshot + VLM ground + pyautogui works, but pays image tokens per step. Keep for UI-only gaps after API path fails.

## Docs (verified, open)
- Word Application: learn.microsoft.com/en-us/office/vba/api/word.application
- Excel object model: learn.microsoft.com/en-us/office/vba/api/overview/excel/object-model
- PowerShell New-Object COM: learn.microsoft.com/en-us/powershell/module/microsoft.powershell.utility/new-object
- PowerShell MIT; Learn samples MIT; Office app proprietary, automation interface documented.
