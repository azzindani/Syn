# Runbook — Windows bring-up (the half no container can verify)

Target: Windows 10/11 + Microsoft 365 desktop + .NET 8 SDK + WebView2 runtime.

## 1. COM sidecar (live hands)
```
cd data\syn\sidecar-csharp\Host
dotnet build -c Release
.\bin\Release\net8.0-windows\office-host.exe --pipe synhand-excel --app excel
```
- Open `plan.xlsx` in Excel first: the sidecar attaches via GetActiveObject
  (live window), falling back to a headless instance.
- Smoke the pipe with one line:
  `{"method":"read","handle":"excel:plan.xlsx:Sheet1","args":{"selector":"Sheet1!A1:B2"}}`
  expect `{"ok":true,"preview":"grid Sheet1: 2x2"}`.
- Open a modal dialog (e.g. Save As) and repeat: expect
  `{"ok":false,"error":"modal dialog or busy app..."}` — queue survives.
- One sidecar per app: `--app word`, `--app powerpoint` on their own pipes.

## 2. Real model call
```
set HARNESS_API_KEY=sk-or-v1-...
set HARNESS_BASE_URL=https://openrouter.ai/api/v1
harness.exe  ->  send routine "summarize the registry"
```
- Watch the widget feed: `step.start` → streamed tokens → `step.done`.
- Astra check: `route vision` must resolve `openai/gpt-6-astra`, effort low.
- Budget check: set cap 3 in the widget, run 3 routine tasks, 4th auto-pauses.

## 3. Widget bundle
```
cd data\syn\widget\src-tauri && cargo tauri build
```
- Sign the bundle, install, drag it over Excel: it must stay on top,
  keep working while Excel has focus, and die with no orphan when closed.
- Kill-switch drill: latch Kill mid-run → dispatch stops, queue pauses,
  only a fresh session re-arms.

## 4. Mac/Web hand
- Sideload `office-pane/` manifest in Word on Mac or office.com,
  press Read selection → envelope appears in the widget relay log.

## 5. Acceptance (mirrors the Linux E2E)
1. Attach live `plan.xlsx` + `report.docx`.
2. `xfer` Q3 table Excel → Word, visibly typed, preview + undo link in feed.
3. `journal` + `replay` reproduce the run after killing the widget.
4. Exported `plan.xlsx` opens in Excel with values + formatting intact.

Known gaps to close on Windows: installer + auto-update, crash-dump
upload, WebView2 boot check, Office bitness (x64) mismatch guard.
