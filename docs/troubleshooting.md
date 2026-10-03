# Troubleshooting

## The model is told what went wrong

When an application refuses a call, Syn passes on the application's own
message and adds one line saying what it means and what to do. The common
ones:

| Message | Meaning | What to do |
|---|---|---|
| `modal dialog or busy app: human confirm required` | A dialog is open, a cell is being edited, or the application is busy. The call was cancelled and nothing changed. | Close the dialog or press Esc in the application, then retry. |
| `com 0x800706BA` / `0x80010108` / `0x800706BE` | The application closed or crashed. | From MCP, call `open` again, which restarts what is needed. In the console, restart the helper and reconnect the app. |
| `workbook not open for …` / `doc not open for …` | The document was closed. | `open` it again. |
| `com 0x800A03EC` | Excel refused without saying why: most often a formula in a local language or with `;` separators, a sheet name that does not exist, or a protected sheet. | Write formulas in English with commas; check the sheet name; ask the user to unprotect. |
| `the document changed after Syn's last edit …` | `undo` refused because someone edited the document since. | Undo in the application itself (Ctrl+Z), or ask the person. |
| `… is Excel only; Word has no …` | The verb does not exist in that application. | Use one of the verbs listed in the message. |
| `same op+args 3x … human confirm required` | The same call was made three times in a row. | Look at the document, then change the approach. The next message from a person resumes the run. |

## Setup problems

**`office-host.exe` does not attach to my open workbook.** The helper and
Office must run as the same user, at the same privilege level: an elevated
(administrator) helper cannot see a normal Excel, and vice versa. Office COM
servers are single-instance per user, so the helper attaches to the running
application if there is one.

**Nothing happens and the call times out.** A dialog in the application
(Save As, a Protected View bar, a sign-in prompt) blocks every COM call into
it. Close it. Documents opened from the internet start in Protected View and
cannot be edited until the user enables editing.

**Excel refuses VBA with "no VBA project".** Turn on *Trust access to the VBA
project object model* (File → Options → Trust Center → Trust Center Settings →
Macro Settings), and start Syn with `AGENT_VBA=1`.

**The console says no apps are connected.** The console connects to helpers
that are already running and named in `.env` (`AGENT_PIPE_EXCEL` and so on);
see [setup-windows.md](setup-windows.md#run). An MCP client starts helpers by
itself.

**A long model reply stops with "the model sent nothing for 180s".** The
connection went silent. Raise `AGENT_STREAM_IDLE_SECS`, or set
`AGENT_STREAM=0` if the provider or a gateway in between mishandles
streaming.

**Rate limits (429).** Syn waits and retries the same model once, then walks
the other model slots. Give the slots different models, ideally on different
providers, or pick another model in the console.

## Collecting details

- `AGENT_TRACE=1` (or `--trace`) makes a helper print every step of every
  call to stderr. When a call never returns, the trace shows how far it got.
- The CLI's `journal <path>` records every command; `replay <path>` runs them
  again.
- Live progress logs are in `.agent/live/`, conversations in `.agent/chats/`.

## Talking to a helper directly

To take Syn out of the picture, start a helper and send it one request:

```
sidecar-csharp\Host\bin\Release\net8.0-windows\office-host.exe --pipe hand-excel --app excel --trace
powershell -File scripts\pipe-client.ps1 -Pipe hand-excel -Json '{"method":"read","handle":"excel:plan.xlsx:Sheet1","args":{"selector":"Sheet1!A1:B2"}}'
```

Expect `{"ok":true,"preview":"grid Sheet1: 2x2 = ..."}`. With a dialog open in
Excel (Save As, say) the same request answers `{"ok":false,"error":"modal
dialog or busy app ..."}` and the helper keeps serving.

## Lessons behind the helper's design

Each of these cost a debugging session on real Office, and each is why the
C# helper is written the way it is:

1. **`Marshal.GetActiveObject` does not exist in .NET Core and later.** Live
   attach uses `CLSIDFromProgID` and `oleaut32!GetActiveObject` directly.
2. **A COM call cannot be timed out on a worker thread.** Blocking the owning
   STA while another thread makes the call deadlocks, because the marshalled
   call needs the owner to pump messages. Calls run inline under an
   `IOleMessageFilter`, which retries a busy application and cancels once
   the time budget is spent.
3. **A message-mode pipe server never completes a read from a byte-mode
   client, and `Encoding.UTF8` writes a byte-order mark first.** Both ends use
   byte mode and `UTF8Encoding(false)`.
4. **Quitting an application with an unsaved document raises a modal prompt
   that blocks every COM call, including the quit.** And `New-Object` on a
   running Word hands back the user's own instance, so a script that quits
   it closes their work. Syn never quits an application, closes only a
   document it opened and has saved (`struct` `close`), saves only when
   asked (`struct` `save`, never Save As), and the test fixture script
   refuses to run while Office is open.
5. **The application can go away under a helper.** When the user quits Word,
   or it crashes, every call through the old reference fails with
   `0x800706BA` (RPC server unavailable) or `0x80010108` (disconnected), and
   a helper that kept it failed that way for the rest of its life. A call
   that failed that way never reached the application, so the helper lets
   go of the dead reference, attaches to the running application (or starts
   one), drops what it knew about the old one's documents, and makes the
   call once more. `0x800706BE`, which can mean it died partway through a
   call, is not retried.
6. **A helper stopped hard leaves its undo scratch workbook behind**, hidden
   and never saved, because it never reaches its own clean-up. Each scratch
   workbook carries a `SynUndoScratch` property naming its helper's process,
   and an Excel helper starting up closes the ones whose helper is gone;
   a workbook without that property is never touched. The scratch
   workbook's window is hidden as well as the workbook: `Visible = false`
   alone left an empty frame titled "Excel" on the desktop beside the
   person's own.
   Worksheet.Copy into a workbook whose window is hidden is refused for any
   sheet ("Unable to get the Copy property of the Worksheet class"), which
   made every undo of a deleted sheet fail; the window is shown for the copy
   alone, with screen updating off.
   The workbooks an export makes along the way (`Sheets.Copy()` of a CSV,
   a copy opened to be saved as xlsx) get a frame of their own that is on
   screen until the call returns, a second or more for a large file; a
   watcher thread hides new frames of that Excel while the call runs, as the
   VBA watcher does for its dialogs. Hide only the frame there, never the
   workbook's window: Excel writes a window's visibility into the file, and
   an xlsx saved from a workbook whose window was hidden opens hidden, with
   nothing on screen.
7. **Word counts a table's cells as paragraphs**, and one more at the end
   of every row, so a 12x4 table is sixty `p` numbers. Shown one by one they
   read as loose lines, and a model deleted real tables three times taking
   them for fake ones. `read` shows a table as one entry naming its
   paragraphs, and a table inserted in front of a heading is made Normal
   first, or every cell takes the heading's style.
8. **A macro that fails waits for a person.** VBA answers a run-time error
   with its Continue / End / Debug box, and `Application.Run` does not
   return until someone clicks; the message filter cannot cancel a call
   that is not rejected, only unfinished. The first live division by zero
   held the run for minutes and came back as `0x800A9C68`. While a macro
   runs, a watcher thread finds VBA's own dialog in that Excel (class
   `#32770`, title `Microsoft Visual Basic`), reads its text, and posts a
   click to End (OK for a compile error). A compile error met mid-run also
   leaves VBA paused in the debugger, so the watcher then presses Run >
   Reset through its own COM connection. Compiling first would be tidier,
   but Debug > Compile reports itself disabled while the editor is hidden.
9. **Excel splits a CSV on its own list separator, not on commas.** With
   the decimal separator set to "," the list separator is ";", and
   `Workbooks.Open` on a comma file of 483,054 rows gave one column of
   whole lines; a two-column line such as `1,5` became the number 1.5.
   `Local` changes nothing, and `OpenText` ignores its delimiter for a file
   named `.csv` (it honoured it once, for a test file with a byte-order
   mark and CRLFs, which made it look like the fix). The helper opens the
   file as usual, so the workbook stays that file, then refills the sheet
   with a text `QueryTable` given the delimiter read off the file's first
   line, and deletes the query. The same file exported to xlsx used to
   lose its data: `SaveCopyAs` on a workbook read from text writes text,
   one sheet, so a text workbook is now exported by copying all its sheets
   into a new workbook.
10. **Word refuses a chart pasted in the chart formats.** `PasteAndFormat`
    with `wdChart` (14) or `wdChartLinked` (15) answers 0x800A11FD "This
    command is not available" for an Excel chart on the clipboard that pastes
    fine every other way, so `embedChart` had never worked live. A plain
    `Range.Paste()` of the chart is a real chart (`HasChart`, linked to the
    workbook), and `Chart.ChartData.BreakLink()` makes it a copy; the helper
    pastes plain when the chart formats are refused.
11. **PowerPoint's `ExportAsFixedFormat` is refused through late binding**
    ("Could not convert argument 0"). A deck is exported as a PDF with
    `SaveCopyAs(path, 32)`, which writes the file and leaves the open deck,
    and its path, alone. A deck with no slides cannot be saved as a PDF at
    all.
12. **A call that never comes back hangs the whole turn.** Excel answers a
    COM call only when it is done, so a formula that compares every row
    with every other (`SUMIF` with a whole column of criteria over 85,000
    rows, for 58 cells) kept a write waiting for 22 minutes, twice in one
    50-message run, and each time someone had to kill Excel, which takes
    the person's other workbooks with it. `Watchdog.cs` watches from the
    listener threads, which use only Win32: past `AGENT_OFFICE_CALL_SECS`
    (120) it presses Esc at Excel, which stops a calculation as it does for
    a person (probed: an Esc at 8 s ended a 29 s calculation at 8.7 s, with
    `CalculationState` left pending); the call's change is then undone with
    calculation switched to manual around it, because the formula that
    caused it would start the calculation again; and after a further 60 s
    the caller is answered anyway. Calls that are slow by nature (`open`,
    `export`, `save`, `close`) get `AGENT_OFFICE_LONG_SECS` (1200) and no Esc.
13. **A box on screen that only a person can answer.** Opening a workbook
    that links to others raised "This workbook contains links to one or more
    external sources that could be unsafe", and the `open` waited an hour.
    `Workbooks.Open` now passes `UpdateLinks:=0`, so it never asks (linked
    cells keep the values saved in the file, and the reply says the workbook
    has links). Any other box is read through UI Automation, by window
    handle (hit-testing points on the screen read whatever was on top of the
    box, and returned nothing once something was), and reported to the
    caller with its text and buttons after 10 s; the one box answered for
    the caller is the links one, with Don't Update. The older message
    filter (`Guarded`, 15 s) still answers calls Excel *refuses* while a
    box is up; this covers calls Excel *accepts* and never finishes.
14. **Gathering files into one workbook turned empty cells into 0.** With
    no way to copy a range from another open workbook, a model wrote
    `=trips.csv!A1:L85411` formulas and pasted them as values: every empty
    cell came across as 0 (1,714 trips with no driver became driver "0"),
    dates came across as serial numbers, and the saved file kept external
    links. `copy` takes `[Book.csv]Sheet!A1:D9` as its source; it is
    `Range.Copy` cell to cell, so blanks stay blank and formats come along.
15. **A cell left in edit mode makes Excel refuse everything, with nothing on
    screen to find.** The status bar reads "Cell Mode Enter" (or Edit, or
    Point); every call is answered 0x800AC472 or rejected until the message
    filter gives up at 15 s. A 50-message run lost seven turns to it: the
    model ended each with "press Esc in Excel", and nobody was there. What
    started the edit is not known (a person clicking in the window is one
    possibility). `CellEdit.cs` reads the mode through UI Automation when a
    call is refused that way, and presses Esc itself, then repeats the call,
    only when the keyboard and mouse have been untouched for
    `AGENT_OFFICE_IDLE_SECS` (60). Otherwise the refusal says in words that
    someone is typing in a cell and what to do. Esc throws away only the
    unfinished edit. Checked live by typing into a cell through
    `Application.SendKeys` and calling through `mcpgate` both ways.
16. **An empty frame titled just "Excel" stayed on screen while a big CSV
    opened.** Opening a 170,000-row file takes 5 to 10 s counting the import
    that splits its columns, with screen updating off, so the new frame was
    never painted and sat there blank: the window that blinked up for the
    person. The whole open runs with new frames (and any frame still titled
    "Excel") hidden, and they are shown again, without taking focus, when the
    workbook is in. Measured with the 30 ms window sampler: 8.9 s and 4.9 s
    became 0.3 s and 0.04 s. A plain `Workbooks.Open` is fast (1.3 s for the
    196,000-row file) and the hide call returns in 55 ms; what cannot be
    hidden is a window whose thread is not pumping messages, which is why the
    hide must not be left to the end.
