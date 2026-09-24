# Runbook — Windows bring-up (the half no container can verify)

Target: Windows 10/11 + Microsoft 365 desktop + .NET 8 SDK + WebView2 runtime.

## 0. Status: what is actually verified

Verified 2026-09-19 on Windows 11 + Microsoft 365 + .NET 8.0.425:

- sidecar **compiles** (it never had been) and attaches to a workbook the
  human already has open, via a hand-rolled ROT lookup;
- `read`, `write` and the failure paths answer over the named pipe against
  **live Excel**, and a write is confirmed present in the running instance;
- the sidecar detaches without closing the user's Excel, leaving no orphan.

Run it yourself: `scripts\live-excel-smoke.ps1` (prints PASS/FAIL, transcript
in `testbed/logs/`). Fixtures come from `scripts\new-testbed-docs.ps1`.

Since then, also verified live:

- the Rust core drives the sidecar over the pipe (`core::hand::Hand`, §6);
- Word and PowerPoint dispatch, `format`, `struct` and `export`, all
  exercised by the capability test's scripted control
  (`tests/capability/load.txt`: 217 operations over Excel, Word and
  PowerPoint, every figure exact — see `tests/capability/SPEC-v3.md`).

**Not yet verified live:** snapshot `.bak` and the Tauri bundle.

### Four things real Office taught us that no container could

1. **`Marshal.GetActiveObject` does not exist off .NET Framework.** It is the
   entire live-attach story and it was the first compile error. Replaced with
   `CLSIDFromProgID` + `oleaut32!GetActiveObject`.
2. **A COM call cannot be timed out by running it on a worker thread.** The
   original guard started a second STA thread and blocked the first on an
   event; every request against live Excel deadlocked, because the marshalled
   call needs the owning apartment to pump messages and the owner was parked
   in a non-pumping wait. Calls now run inline under an `IOleMessageFilter`,
   which retries a busy server and cancels once the budget is spent.
3. **A message-mode pipe server never completes a read from a byte-mode
   client**, and `Encoding.UTF8` prepends a BOM to the first line on the
   wire. Both ends are now byte mode with `UTF8Encoding(false)`.
4. **Quitting an Office app with a dirty document raises a modal save prompt,
   and a modal dialog blocks every COM call into that app — including the
   Quit that raised it.** `DisplayAlerts` does not suppress it. Mark documents
   `Saved` before quitting. Relatedly, Office COM servers are effectively
   single-instance per user: `New-Object` on a running Word hands back the
   user's own session, so a script that quits it closes their work. The
   fixture generator refuses to run while Office is open for exactly this
   reason, and the sidecar never quits what it did not start.

## 1. COM sidecar (live hands)
```
cd sidecar-csharp\Host
dotnet build -c Release
.\bin\Release\net8.0-windows\office-host.exe --pipe hand-excel --app excel
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
set AGENT_API_KEY=sk-or-v1-...
set AGENT_BASE_URL=https://openrouter.ai/api/v1
cli.exe  ->  send routine "summarize the registry"
```
- Watch the widget feed: `step.start` → streamed tokens → `step.done`.
- Routing check: `route vision` must resolve the reasoning slot
  (`AGENT_MODEL_REASONING`, default `openrouter/auto`), effort low.
- Budget check: set cap 3 in the widget, run 3 routine tasks, 4th auto-pauses.

## 3. Widget bundle
```
cd widget\src-tauri && cargo tauri build
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

## 6. Brain to hand, end to end

The seam is closed, and live ops take the supervised route. `core::hand::Hand`
speaks `office-rpc/1` over the pipe; `Runner` dispatches a handle marked live
through it, passing the same kill switch, app allowlist, doom-loop gate,
registry check and event feed as an in-memory op:

```
cli.exe
  attach excel plan.xlsx Sheet1
  hand   hand-excel
  live   excel:plan.xlsx:Sheet1
  lread  excel:plan.xlsx:Sheet1 Sheet1!A1:C5
  write  excel:plan.xlsx:Sheet1 Sheet1!G1 queued,through,runner
```

Verified 2026-09-19 against live Excel. Reads and the write landed
(`G1:I1 = queued through runner`, read back from the running instance), the
feed showed `step.start`/`step.done ... live` for each, and the safety
machinery provably held: after `allow word` the live write was refused with
`app "excel" not on allowlist`, and after `kill` the next one was refused
with the latch error. **Neither cell was written** — the guards stop live
document edits, not just in-memory ones.

Deliberate gaps: no relay snapshot is taken for a live handle (undo belongs
to the sidecar's `.bak` plus the app's own stack, and a fake snapshot would
make `undo` look available when it is not); a verb a sidecar does not
implement refuses rather than silently no-ops; a dead pipe drops the
hand and freezes the queue so the next op cannot quietly fall back to the
in-memory model and report success for a document nobody touched.

Known gaps to close on Windows: installer + auto-update, crash-dump
upload, WebView2 boot check, Office bitness (x64) mismatch guard.
