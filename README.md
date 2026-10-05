# Syn

Syn is a desktop agent for one person on one machine. It lets a language
model work in the applications you already have open — Word, Excel and
PowerPoint through COM, any native Windows window through UI Automation, and
web pages in a browser of its own, and Electron apps, through the Chrome
DevTools protocol — while you watch, interrupt and undo.

It reads structure, not pixels: no screenshots and no clicking by
coordinates. Any OpenAI-compatible model works, through OpenRouter by
default, and any MCP client can use the same tools.

## What it does

- **Works in your open documents.** It attaches to the Excel, Word or
  PowerPoint you have running and edits the documents in front of you, or
  opens files itself.
- **Six operations for everything:** `read`, `write`, `format`, `struct`,
  `export` and `undo`, on handles like `excel:plan.xlsx:workbook`. `struct`
  covers what a person does with the ribbon: rows and sheets, sort and
  filter, find and replace, tables, pivots, charts, validation, comments,
  links, headers and page setup; paragraphs, tables and contents in Word;
  slides, text boxes, themes and slide size in PowerPoint. See
  [docs/tools.md](docs/tools.md).
- **Browses in a window of its own.** With Chrome or Edge on the computer
  and nothing to set up, it opens pages, reads what can be pressed or
  filled, types and clicks with real input, follows links into new tabs,
  and waits for pages that load late. The browser keeps its own profile, so
  you sign in to a site in its window once, and it never types a password
  or a card number: it asks you to. It is not your everyday browser, and it
  goes when Syn does. `AGENT_BROWSER=off` removes it.
- **Real formulas, not arithmetic in the model.** It writes live formulas and
  reads their results, so a sheet of 250,000 rows is one call, not a
  context window.
- **Undo per document**, including in Excel, where COM changes normally
  bypass Excel's own undo.
- **Exports are copies**: xlsx, csv, docx, pptx, pdf, and charts or slides
  as png, without moving the file you have open.
- **Guarded.** Every call, from any client, passes the same kill switch, app
  allowlist, repeated-call gate and closed schemas. What it reads is treated
  as data, never as instructions. Programs and VBA are off unless you turn
  them on, and a human approves each program it runs. See
  [docs/security.md](docs/security.md).

## Three ways to use it

- **The console** — a local web app: pick a model and a thinking level, ask,
  and watch each step as a row in the conversation.
- **Any MCP client** — Claude Desktop, Claude Code, OpenCode, Cursor: point
  it at `mcpgate.exe` and the model gets `status`, `open`, the six
  operations and `manual`. See [docs/mcp.md](docs/mcp.md).
- **The CLI** — the same agent and operations in a terminal.

## Quick start (Windows)

**The installer**, needing only Windows 10/11 and desktop Office: download
`Syn-<version>-setup.exe` from Releases and run it (no administrator
rights needed), start Syn from the Start menu, and add an API key with the
key icon in the sidebar. A zip of the same files is there too, for anyone
who would rather not install. To build both yourself:
`powershell -File scripts\package.ps1` (with NSIS for the installer).

**From source**, needing Rust and the .NET 8 SDK as well:

```
cd core && cargo build --release && cargo build --bins && cd ..
dotnet build -c Release sidecar-csharp\Host
copy .env.example .env        # then set AGENT_API_KEY, or add a key in Settings
```

Then either point an MCP client at `core\target\release\mcpgate.exe`, or
start the console (`powershell -File scripts\console.ps1`) and ask for what
you want: it finds and opens files, and starts the Office helpers, by
itself. See [docs/setup-windows.md](docs/setup-windows.md).

To check an installation against real Office:

```
powershell -File scripts\new-testbed-docs.ps1     # test documents (close Office first)
powershell -File scripts\live-office-peak.ps1     # every verb, one PASS/FAIL line each
```

## Developing on any platform

The core builds and tests anywhere, with no key, no Office and no network:

```
cd core
cargo clippy --all-targets -- -D warnings
cargo test
```

With LibreOffice installed, the whole stack runs live on Linux or macOS
through a LibreOffice helper that speaks the same protocol as the Office one.
See [docs/development.md](docs/development.md).

## Documentation

| | |
|---|---|
| [Setting up on Windows](docs/setup-windows.md) | Build, configure, run, check. |
| [The console and the CLI](docs/console-and-cli.md) | Using Syn's own agent. |
| [MCP](docs/mcp.md) | Using Syn from an MCP client. |
| [Tools reference](docs/tools.md) | Every operation and verb, by application. |
| [Configuration](docs/configuration.md) | Every setting. |
| [Security](docs/security.md) | The gates, and what stays your responsibility. |
| [Troubleshooting](docs/troubleshooting.md) | Errors and fixes. |
| [Architecture](docs/architecture.md) | How it fits together. |
| [Development](docs/development.md) | Tests and adding capabilities. |

## Status

Verified against real Microsoft Office on Windows 11: attaching to open
documents, reading and writing, the gates, and a 217-operation scripted run
across Excel, Word and PowerPoint (`tests/capability/`). The complete verb
set, undo and exports are verified live against LibreOffice on every push;
`scripts\live-office-peak.ps1` is the check that proves them on Office.

## License

MIT. See [LICENSE](LICENSE).
