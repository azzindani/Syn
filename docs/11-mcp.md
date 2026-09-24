# 11 — Syn as an MCP server

Date: 2026-09-24
Status: how-to. Built and tested offline (tier 1); the live path needs a
check on Windows with Office (tier 3), see the end.

`mcpgate` puts Syn's hands behind MCP, so any MCP client — Claude Desktop,
Claude Code, OpenCode, Cursor, or a small open model behind one of them —
can open documents in Excel, Word and PowerPoint and drive them live,
through the same gates Syn's own loop uses. This is phases 1 and 2 of
`09-splitting-harness-and-tools.md`, plus the one thing that plan left out:
an outside model has to be able to *get* a document open, not only edit one
somebody else opened.

## Connecting a client

Build it once: `cd core && cargo build --release`. The binary is
`core\target\release\mcpgate.exe`. It reads Syn's `.env` from its working
directory or from beside the executable, so it does not matter where the
client starts it.

**Claude Desktop** — `claude_desktop_config.json`:

```json
{ "mcpServers": { "syn": { "command": "C:\\src\\Syn\\core\\target\\release\\mcpgate.exe" } } }
```

**Claude Code** — `claude mcp add syn -- C:\src\Syn\core\target\release\mcpgate.exe`

**Any other client** — a local stdio server whose command is that path, with
no arguments.

Try it by hand: `printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | mcpgate`.

## What a model gets

Nine tools, in this order:

| Tool | Does |
|---|---|
| `status` | What is open and the exact handle for each, what can be opened, the next step. The first call also carries the working rules. |
| `open` | Opens a file in Excel, Word or PowerPoint — starting the helper, and through it the application, if they are not running — or finds one already open, or registers a browser page or a window. Returns the handle and what is inside (a workbook's sheet names). |
| `read` `write` `format` `struct` `export` `undo` | The six document ops, generated from `tools::TOOLS`: the same names, descriptions, worked examples and JSON Schemas the loop uses. |
| `manual` | The Excel, Word or PowerPoint reference page. |

Not offered: `shell`, which always stops for a human, whose approval
prompt lives in Syn's own app; and `plan` and the `loop` manual page, which
are about Syn's own budget.

## Built for small models

A frontier model finds its way around almost any tool surface. A 7B–30B
model reaches for the tool whose name sounds like the request, with a
handle it invented. Everything below is aimed at that model:

- **It is told what to do first, three ways.** The server `instructions`
  (injected by clients that honour them), the `status` description ("START
  HERE"), and the first `status` result, which carries the rules for
  clients that never show `instructions` at all.
- **Every result says what to do next, as a call to copy.** `open` ends
  `Next: read{"handle":"excel:plan.xlsx:workbook","selector":"data!A1:H20"}`,
  with a real sheet name, quoted when it has a space. A test runs that line
  exactly as written. A `write` ends with the `read` that checks it.
- **Every refusal says how to fix it.** A missing field comes back with the
  tool's own worked example. An unknown field is named with the real ones.
  A wrong handle comes back with the list of open handles. A bad selector
  comes back with that app's selector grammar. A repeated call is told to
  look at the document and change approach, not just refused.
- **The shapes small models produce are accepted when their meaning is
  certain.** `[["a",1],["b",2]]` becomes `a|1;b|2`; a number becomes its
  text; `plan.xlsx`, or `excel:plan.xlsx:Sheet1` copied from an example,
  resolves to the one open handle it can mean; `"PowerPoint"`, `"pptx"`
  and `"slides"` all mean the same app. What is ambiguous is refused with
  the alternatives: a flat list of values could be a row or a column, so
  it is not guessed.
- **An application's refusal comes back with what it means.** office-host
  passes on what COM said, and COM says `com 0x800A03EC`. `core/src/coach.rs`
  adds one line after the application's own words for each known failure:
  - a dialog open or Excel busy: ask the user to press Esc;
  - the app closed or crashed: call `open`;
  - a protected sheet or a read-only file: ask the user;
  - a formula Excel cannot parse: write it in English with commas;
  - a window control or page element that isn't there: read `:tree` or
    `body` first.

  A test checks that each rule still matches its helper's wording.
- **The selector format sits beside the handle.** `status` shows each open
  app's selector format next to its documents, so a model sees how to
  address Excel or a window before its first call, not after a miss.
- **Windows and pages have manuals too.** `manual` offers `windows` (the
  UI Automation control tree, `id=`/`name=`/`type=`, invoke, toggle,
  select) and `browser` (CSS selectors, filling fields so the page's own
  scripts notice, pages that change under you). The Office pages say what a
  live Windows app does: English formulas, busy and modal states, Protected
  View, slide-show mode. Every page says a button that deletes, sends,
  pays or signs in is the user's to press.
- **The instructions are short.** 359 words, and a test keeps them under
  520: a model with an 8k window pays for them on every turn.

## Security

The rules in `08-production-grade.md` now hold for an outside caller, not
only for Syn's own loop:

- **One road.** Every document op goes through `Runner::run`: the kill
  switch, the app allowlist, the VBA gate (`AGENT_VBA=1` or refused), the
  repeated-call gate, the registry check. The old gateway's writes met
  none of them.
- **Results are data.** Everything read from a document comes back inside
  `<user_content>` with a line saying to treat it as data. Text that looks
  like an instruction ("ignore all previous instructions", `[system:` ...)
  adds a WARNING line. `core/src/mcpgate.rs` has a test in which a cell
  says exactly that.
- **Closed schemas, enforced here.** `additionalProperties:false` on every
  tool, unknown fields refused, caps checked on what arrives (`maxLength`
  is left off the wire because some backends reject the keyword).
- **The model picks an app, never a program.** Which helper serves Excel
  comes from `.env` or the repository's own build output. The model chooses
  from a closed list.
- **Nothing closes, nothing saves.** `open` is idempotent and the helper
  never touches what it did not open. Helpers this server started are
  stopped when it exits; the applications and the human's documents stay.

Settings for the human, in `.env`:

| Variable | Effect |
|---|---|
| `AGENT_MCP_APPS` | Only these apps, e.g. `excel,word`. Checked before anything is launched. A name that is not an app allows nothing. |
| `AGENT_MCP_ROOTS` | `open` only takes files under these folders, `;`-separated. |
| `AGENT_MCP_LAUNCH` | `0`: connect only to helpers that are already running. |
| `AGENT_OFFICE_HOST`, `AGENT_UIA_HOST` | Where the helpers are, if not in this repository's Release build. |
| `AGENT_VBA` | `1` to allow `struct` `macro` for the session. Off by default. |

## More than one client at once

Claude Desktop, OpenCode and the Syn console can all be open at once, each
with its own `mcpgate` (or CLI), driving the same Excel. Each helper serves
up to eight clients: requests interleave one call at a time, and
office-host still makes every COM call from its one STA thread. What one
session writes, the others read. Before this, a helper served one client,
so a desktop app holding Excel all day locked everyone else out, and the
second client's `open` just hung.

If a helper dies, every session using it gets a broken-connection error
that says to call `open` again. `open` reconnects (starting a new helper if
needed), and the session carries on. The call that failed does not count
towards the repeated-call limit. A helper stops when the client that started
it exits, so the other sessions see that as a broken connection and recover
the same way.

## Watching it

Every call an outside model makes is written to Syn's live log, in the same
shape the loop writes. Open the Syn console (`scripts\console.ps1`) beside
the client and the run appears there as it happens: a "Working" card named
after the client, one row per call, in each app's colour. That answers the
limitation `09` §3 expected to have to live with.

## Protocol

JSON-RPC 2.0 over stdio, one message per line. Both eras are spoken,
because clients in the wild are on both:

- **2026-07-28 (modern):** `server/discover`; the version and client in
  every request's `_meta`; `resultType`, `serverInfo` and cache hints on
  every result; `-32022` with the supported list for a version it does not
  speak.
- **2025-11-25 and earlier:** the `initialize` handshake, echoing the
  client's version when it is one of ours.

Tool failures are results with `isError: true`, so the model sees them and
can correct itself; only an unknown tool or a malformed request is a
JSON-RPC error. The JSON is parsed properly (`core/src/json.rs`) — the
substring reader used elsewhere would read `_meta.clientInfo.name` as the
tool name.

## What is verified, and what is not

**Live, against a real office engine (tier 2½), 2026-09-24.** On Linux,
with `sidecar-lo/lo_host.py` standing in for office-host.exe: a model (the
session's own, acting as the MCP client, deciding each call from the last
answer) drove the real `mcpgate` through a full job. `status` → `open` (which
started the helper, which started LibreOffice, 2.1s) → the suggested `read`,
copied exactly → a revenue column filled down 24 rows by one formula → a
`Summary` sheet of `SUMIF`s written as nested arrays against a prefix-less
handle → formatting → two charts → a Word memo with a heading and a table →
a slide with a sub-bullet and speaker notes → export of all three. The
exported files were then checked by unzipping them, with the expected
numbers recomputed from the fixture's own data: every formula was in the
file as a formula, every value matched. The mistakes a small model makes
(a selector with no sheet, an invented field, a repeated call, a file in the
wrong app, a path outside `AGENT_MCP_ROOTS`) each came back with the fix in
the message. Closing the client stopped all six helper processes. The Syn
console, open beside it, showed every call as it happened.

That run found three real bugs, all fixed:

- Excel's `SplitRange` kept the quotes of `'Q3 sales'!A1`, so the selector
  the guidance tells models to write for a sheet with a space would have
  failed on real Excel. Fixed in the C# (unverified there) and in the
  LibreOffice helper (verified).
- The guidance said `p1, p2 ...` for Word paragraphs; both helpers count
  from `p0`. A model following it skipped the first paragraph. It now says
  p0 is the first.
- A label ending in "…" got a full stop after it.

`tests/test_lo_live.py` repeats that job automatically, and CI's
`libreoffice` job runs it on every push with LibreOffice installed. It also
runs two MCP sessions against the same helpers at once: both drive one
workbook and read each other's writes, one holds Excel, Word and PowerPoint
while the other works, and a helper killed mid-session is recovered by
`open`.

Verified here (Linux, tier 1): 36 new tests. They cover the protocol in
both eras, the surface generated from `tools::TOOLS` with a drift check,
every gate on the MCP path, the small-model repairs and refusals, `open`
against fake hands (connect, launch, open, bind, sheet discovery,
refusals before launch), and the real binary over stdio. The crate also
type-checks and passes clippy for the Windows target.

**Not verified: any of it against Microsoft Office.** The Windows launch
of office-host.exe, the handle-inheritance fix, the quote fix in the C#,
the sheet-name probe against live Excel, and opening a file through the C#
sidecar all need tier 3. To check on the desk:

1. Build the sidecar (`dotnet build -c Release sidecar-csharp\Host`) and
   `mcpgate` (`cargo build --release`).
2. Set `AGENT_PIPE_EXCEL=hand-excel` in `.env`, close Excel, and point a
   client at `mcpgate.exe`.
3. Ask it to open a workbook by full path and read the first rows. Expected:
   Excel starts, the workbook opens visibly, `open` reports its sheet names.
4. Quit the client. Expected: `office-host` is gone, and Excel is still open
   with the workbook in it.
