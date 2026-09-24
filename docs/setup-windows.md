# Setting up on Windows

Syn drives Word, Excel and PowerPoint through COM, which needs Windows and a
desktop install of Microsoft Office.

## Requirements

- Windows 10 or 11
- Microsoft Office desktop apps (Microsoft 365 or a perpetual version)
- [Rust](https://rustup.rs) (stable)
- [.NET 8 SDK](https://dotnet.microsoft.com/download/dotnet/8.0)
- PowerShell 5.1 or later (included with Windows)
- For browsers and Electron apps: Chrome or Edge
- For the model-driven agent: an API key for an OpenAI-compatible provider
  (OpenRouter by default). Not needed to use Syn from an MCP client.

## Build

From the repository root:

```
cd core
cargo build --release        # cli.exe, ui.exe, mcpgate.exe
cargo build --bins           # debug builds, used by scripts\console.ps1
cd ..
dotnet build -c Release sidecar-csharp\Host   # office-host.exe (Office)
dotnet build -c Release sidecar-csharp\Uia    # uia-host.exe (any window)
```

## Configure

```
copy .env.example .env
```

Then edit `.env`:

```
AGENT_API_KEY=sk-or-v1-...          # for the console and the CLI's agent
AGENT_PIPE_EXCEL=hand-excel
AGENT_PIPE_WORD=hand-word
AGENT_PIPE_PPT=hand-powerpoint
AGENT_PIPE_UIA=hand-uia
```

Everything else has a working default; see [configuration.md](configuration.md).

## Run

**From an MCP client** (Claude Desktop, Claude Code, OpenCode, …): point the
client at `core\target\release\mcpgate.exe`. It starts the Office helper for
an application the first time it is needed. See [mcp.md](mcp.md).

**The console** (chat with a model that works in your open apps) connects to
helpers that are already running. Start one per application, each in its own
window, then the console:

```
$h = "sidecar-csharp\Host\bin\Release\net8.0-windows\office-host.exe"
Start-Process $h "--pipe hand-excel --app excel"
Start-Process $h "--pipe hand-word --app word"
Start-Process $h "--pipe hand-powerpoint --app powerpoint"
powershell -File scripts\console.ps1            # add -WithUia for native windows
```

The console prints its address (`http://127.0.0.1:7777/`) and opens it; the
status menu at the top right connects the apps. See
[console-and-cli.md](console-and-cli.md).

A helper attaches to the application if it is already running, so Syn works
on the documents you have open, and starts it otherwise. It never closes an
application or saves a document it did not open itself.

## Check the installation

These scripts work on throwaway documents in `testbed\`, never on yours.

```
powershell -File scripts\new-testbed-docs.ps1   # make plan.xlsx, report.docx, deck.pptx (close Office first)
powershell -File scripts\live-excel-smoke.ps1   # attach to Excel, read, write, detach
powershell -File scripts\live-office-peak.ps1   # every Excel, Word and PowerPoint verb through mcpgate
```

`live-office-peak.ps1` prints one PASS or FAIL line per step and exits with
the number of failures. `-Vba` adds the VBA steps (see
[security.md](security.md#vba)); `-Theme <file>` adds applying a theme;
`-SkipDocs` reuses the existing test documents. Exports land in
`testbed\out`, and the applications are left open so you can look at what
each step did.

Also available: `live-uia-smoke.ps1` (drives Calculator through UI
Automation) and `live-cdp-smoke.ps1` (drives a browser through the DevTools
protocol).

## Stopping

```
Get-Process ui, uia-host, office-host | Stop-Process
```

Stopping a helper never closes Office or your documents.

If something does not work, see [troubleshooting.md](troubleshooting.md).
