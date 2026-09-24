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

Verified here (Linux, tier 1): 36 new tests. They cover the protocol in
both eras, the surface generated from `tools::TOOLS` with a drift check,
every gate on the MCP path, the small-model repairs and refusals, `open`
against fake hands (connect, launch, open, bind, sheet discovery,
refusals before launch), and the real binary over stdio. The crate also
type-checks and passes clippy for the Windows target.

**Not verified: any of it against real Office.** The helper launch
(`EnvConnector`), the handle-inheritance fix on Windows, the
sheet-name probe against live Excel, and opening a file through the
sidecar all need tier 3. To check on the desk:

1. Build the sidecar (`dotnet build -c Release sidecar-csharp\Host`) and
   `mcpgate` (`cargo build --release`).
2. Set `AGENT_PIPE_EXCEL=hand-excel` in `.env`, close Excel, and point a
   client at `mcpgate.exe`.
3. Ask it to open a workbook by full path and read the first rows. Expected:
   Excel starts, the workbook opens visibly, `open` reports its sheet names.
4. Quit the client. Expected: `office-host` is gone, and Excel is still open
   with the workbook in it.
