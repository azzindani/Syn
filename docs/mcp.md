# Using Syn from an MCP client

`mcpgate` is an MCP server that lets any MCP client — Claude Desktop,
Claude Code, OpenCode, Cursor, or a small open model behind one of them —
open documents in Excel, Word and PowerPoint (and windows and web pages) and
work on them live, through the same safety gates as Syn's own agent.

## Connect a client

Build it once:

```
cd core
cargo build --release          # core\target\release\mcpgate.exe
```

`mcpgate` reads Syn's `.env` (see [configuration.md](configuration.md)), found
from its working directory or beside the executable, so it does not matter
where the client starts it. At minimum, name the Office helpers' pipes:

```
AGENT_PIPE_EXCEL=hand-excel
AGENT_PIPE_WORD=hand-word
AGENT_PIPE_PPT=hand-powerpoint
```

**Claude Desktop** — in `claude_desktop_config.json`:

```json
{ "mcpServers": { "syn": { "command": "C:\\src\\Syn\\core\\target\\release\\mcpgate.exe" } } }
```

**Claude Code:**

```
claude mcp add syn -- C:\src\Syn\core\target\release\mcpgate.exe
```

**Any other client:** a local stdio server whose command is the path to
`mcpgate.exe`, with no arguments.

To check it by hand:

```
echo {"jsonrpc":"2.0","id":1,"method":"tools/list"} | mcpgate.exe
```

## What the model gets

| Tool | Does |
|---|---|
| `status` | What is open, the exact handle for each and its selector format, what can be opened, and the next step. The first call also carries the working rules. |
| `open` | Opens a file in Excel, Word or PowerPoint, starting the helper and the application if they are not running, or finds it if it is already open. Also registers a window or a web page. Returns the handle and what is inside (a workbook's sheet names). |
| `read`, `write`, `format`, `struct`, `export`, `undo` | The six document operations, identical to the ones Syn's own loop uses. See [tools.md](tools.md). |
| `manual` | Reference pages: `excel`, `word`, `powerpoint`, `windows`, `browser`. |

Not offered over MCP: `shell` (it always needs a human's approval, which lives
in Syn's console) and the loop's own `plan`.

`open` never closes or saves anything, and opening a document that is already
open returns it as it is, so the human's unsaved work is never touched.

## Built for small models

A frontier model finds its way around almost any tool surface; a 7B–30B
model reaches for the tool whose name sounds right, with a handle it made
up. The server is written for that model:

- **What to do first is said three ways:** in the server `instructions`, in
  the `status` description ("START HERE"), and in the first `status` result,
  for clients that never show `instructions`.
- **Every result ends with the next call, ready to copy**, for example
  `Next: read{"handle":"excel:plan.xlsx:workbook","selector":"data!A1:H20"}`
  with a real sheet name. A test runs that line exactly as written.
- **Every refusal says how to fix the call:** a missing field comes back with
  the tool's worked example, an unknown field with the real ones, a wrong
  handle with the open handles, a bad selector with that app's selector
  format, a verb the app lacks with the verbs it has.
- **Shapes small models produce are accepted when their meaning is certain:**
  `[["a",1],["b",2]]` becomes `a|1;b|2`; `plan.xlsx` resolves to the one open
  handle; `PowerPoint`, `pptx` and `slides` mean the same app. Ambiguous
  input (a flat list could be a row or a column) is refused with the
  alternatives, never guessed.
- **An application's refusal comes back explained.** COM says
  `com 0x800A03EC`; Syn adds what it means and what to do: a dialog is open
  (ask the user to press Esc), the app closed (call `open`), the sheet is
  protected (ask the user), the formula must be English with commas.
- **Instructions are short,** because a model with a small context window pays
  for them on every turn. A test caps their length.

## Settings

| Variable | Effect |
|---|---|
| `AGENT_MCP_APPS` | Only these apps, e.g. `excel,word`. |
| `AGENT_MCP_ROOTS` | `open` only takes files under these folders (`;`-separated). |
| `AGENT_MCP_LAUNCH` | `0`: connect only to helpers that are already running. |
| `AGENT_OFFICE_HOST`, `AGENT_UIA_HOST` | Where the helpers are, if not in this repository's Release build. |
| `AGENT_VBA` | `1` to allow `macro` for this server's session. |

The model chooses an application from a closed list; which program serves it
comes only from these settings or the repository's own build output.

## Several clients at once

Claude Desktop, OpenCode and Syn's console can all run at once, each with its
own `mcpgate` or CLI, driving the same Excel. Each helper serves up to eight
clients; their requests interleave one call at a time, and every COM call is
still made from the helper's single STA thread. What one session writes, the
others read.

If a helper dies, each session using it gets an error saying to call `open`
again; `open` reconnects, starting a new helper if needed. Helpers a server
started are stopped when that server exits; the applications and the
documents stay open.

## Watching a client work

Every call an outside model makes is written to Syn's live log. Open Syn's
console beside the client and the run appears there as it happens, one row
per call, in each application's colour.

## Protocol

JSON-RPC 2.0 over stdio, one message per line. Both protocol generations are
supported: the `initialize` handshake (2025-11-25 and earlier) and
`server/discover` with per-request `_meta` (2026-07-28). Tool failures are
results with `isError: true`, so the model sees them and can correct itself;
only an unknown tool or a malformed request is a JSON-RPC error. Standard
output carries protocol only; diagnostics go to standard error.

## Verification status

The whole MCP path — protocol, `open`, every gate, the guidance, and real
documents with formulas, charts and exports — is tested automatically against
LibreOffice on every push (`tests/test_lo_live.py`). The Microsoft Office side
is exercised by `scripts\live-office-peak.ps1`, which drives every verb
through `mcpgate.exe` against real Excel, Word and PowerPoint and prints a
PASS or FAIL line for each. See [development.md](development.md#test-tiers).
