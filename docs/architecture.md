# Architecture

Syn is a desktop agent for one user on one machine. A model — Syn's own agent
loop or any MCP client — works in the applications that are open on the
desktop: Word, Excel and PowerPoint through COM, native windows through UI
Automation, and browsers and Electron apps through the Chrome DevTools
protocol. It reads structure, not pixels: no screenshots, no clicking by
coordinates.

## How a call travels

```
  console (ui) ──stdin/stdout──▶ cli ─┐
  terminal ─────────────────────▶ cli ─┤
  MCP client ──stdio──▶ mcpgate ───────┤
                                       ▼
                              Runner::run  ── the gates ──┐
                                       │                   │
                 ┌─────────────────────┼───────────────┐   │
                 ▼                     ▼               ▼   ▼
        office-rpc/1 (pipe)    office-rpc/1 (pipe)   CDP (WebSocket)   in-memory model
                 │                     │               │
          office-host.exe         uia-host.exe      Chrome / Edge / Electron
          (STA, COM)              (MTA, UIA)
                 │                     │
      Excel · Word · PowerPoint   any native window
```

Every caller reaches a document through **one path**, `Runner::run`
(`core/src/runner.rs`). There is no second entry point; a new one would skip
every gate. On the way, each call passes:

1. **the kill switch** — once latched, nothing more is dispatched;
2. **the app allowlist** — `allow excel word` confines a session;
3. **the VBA gate** — `macro` is refused unless `AGENT_VBA=1`;
4. **the per-app verb table** — a verb the application does not have is
   refused with the list of those it does;
5. **the repeated-call gate** — the same call on the same document three
   times running pauses the run;
6. **the registry check** — only documents that are open can be addressed.

Each call is written to the event feed and to the process's live log, which
is how the console shows runs from any process as they happen.

## Components

### Rust core (`core/`)

A single crate with **no dependencies**: the JSON parser, HTTP server,
WebSocket client and SSE parser are written with the standard library, and
the model provider is reached through `curl`.

| Module | Role |
|---|---|
| `tools` | The tool surface: names, descriptions, closed JSON Schemas, argument parsing, the per-app verb table. The single source of truth for every caller. |
| `ops` | The operations and the in-memory document model (used when no application is connected). |
| `runner`, `queue`, `guard`, `security`, `bus` | The one path to a document and its gates; the event feed and session state. |
| `hand` | The `office-rpc/1` client for the Office and UI Automation helpers. |
| `cdp`, `ws` | The browser hand: Chrome DevTools over an RFC 6455 WebSocket. |
| `desk` | How an MCP session gets a document open: connect a helper, start it if needed, open the file, bind the handle. |
| `mcpgate` | The MCP server; its document tools are generated from `tools`. |
| `agent`, `looptools`, `surface` | The agent loop, the tools it answers itself (`plan`, `manual`), and the merged surface it offers the model. |
| `provider`, `sse`, `router`, `config`, `catalog`, `auth` | Model requests (streamed), the four model slots, `.env` loading, the model list, credentials. |
| `coach` | Turns an application's refusal (a COM error code, a dialog, a missing control) into what to do next. |
| `manual` | The reference pages a model can read. |
| `chats`, `live` | Saved conversations and the live progress logs. |

Binaries: `cli` (the REPL), `ui` (the console's local web server, which
drives `cli`), and `mcpgate` (the MCP server on stdio).

### Helpers

| Helper | Language | Serves | Notes |
|---|---|---|---|
| `sidecar-csharp/Host` (`office-host.exe`) | C#, .NET 8 | Excel, Word, PowerPoint | Late-bound COM on one STA thread; attaches to a running application; every call under a busy/modal guard; keeps the per-document undo record. |
| `sidecar-csharp/Uia` (`uia-host.exe`) | C#, .NET 8 | Any native window | UI Automation on an MTA thread (an STA client can deadlock against an STA provider). |
| `sidecar-lo/lo_host.py` | Python + UNO | Excel, Word, PowerPoint via LibreOffice | The same wire, for Linux, macOS and CI. A test and development tool, never on the Windows path. |

Each helper serves up to eight clients at once. office-host queues every
request to its single STA thread, so COM is only ever called from there.

### Console (`widget/index.html`)

The console is one HTML file with no dependencies, compiled into `ui`. It
posts command lines to the local server, which types them into a real `cli`
process, and renders what the CLI prints. It follows runs through a
server-sent event stream over the live logs.

## The wire: office-rpc/1

One JSON object per line, over a named pipe on Windows (a Unix socket
elsewhere):

```
→ {"method":"read","handle":"excel:plan.xlsx:Sheet1","args":{"selector":"Sheet1!A1:B2"}}
← {"ok":true,"preview":"grid Sheet1: 2x2 = Region|Units;North|40"}
← {"ok":false,"error":"modal dialog or busy app: human confirm required ..."}
```

Short fields ride in `args`; long ones (a grid, prose, VBA source) ride in
`payload`. `core/tests/wire_contract.rs` reads the Rust and C# sources and
fails if a method Rust can send has no handler, or if an app's verb table
disagrees with its dispatcher.

## The agent loop

`core/src/agent.rs` turns a goal into tool calls. The model sits behind the
`Brain` trait, so the whole loop is tested offline with scripted replies. Per
turn the loop:

- sends the conversation and the tool surface to the model, streaming the
  reply and showing it as it is written;
- runs each tool call through `Runner::run`, returning results fenced as
  untrusted data;
- keeps a step budget (shown to the model), compacts the conversation to fit
  the model's context window, and retries or falls back to another model
  slot on rate limits and outages;
- ends when the model answers in prose, the budget runs out, or a gate stops
  it.

Models are chosen by job — `small`, `standard`, `coding`, `reasoning` — each
configurable to any OpenAI-compatible endpoint (see
[configuration.md](configuration.md)).

## Design rules

- **Six operations are the product.** `read`, `write`, `format`, `struct`,
  `export`, `undo` over `app:file:unit` handles, one vocabulary across COM,
  UI Automation and CDP. New capability is a `struct` verb, not a seventh
  operation.
- **Closed schemas.** Unknown fields and verbs are refused; every string is
  capped; oversized input is refused, never truncated.
- **Results are untrusted.** Anything read from a document or a program goes
  back to the model marked as data, never as instructions.
- **Real control is COM on Windows.** No Python office-file libraries, and no
  Python on the Windows control path.
- **Never close or save what Syn did not open.** Office COM servers are
  single-instance per user; quitting one can close the user's own work.

## Repository layout

| Path | Contents |
|---|---|
| `core/` | The Rust core, binaries and tests. |
| `sidecar-csharp/` | `Host` (Office) and `Uia` (UI Automation) helpers. |
| `sidecar-lo/` | The LibreOffice helper and its test fixtures. |
| `widget/index.html` | The console UI. |
| `protocol/` | The default-deny security policy and the original rpc catalog (JSON). |
| `scripts/` | Windows setup and live test scripts (PowerShell). |
| `tests/` | LibreOffice live tests, console (Playwright) tests, the capability test and its recorded runs. |
| `testbed/` | Gitignored scratch area for live runs. |
| `relay/` | The original Python prototype; its tests still run in CI. |
| `office-pane/`, `widget/src-tauri/` | Early prototypes (an Office.js task pane and a Tauri shell). Not built and not part of the product. |
| `docs/design/` | Design notes and digests of other projects that parts of Syn were modelled on. |
