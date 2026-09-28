# package.ps1 - build Syn for Windows as one folder you can unzip and run.
#
#   powershell -File scripts\package.ps1              # dist\Syn-<version>-win-x64.zip
#   powershell -File scripts\package.ps1 -Out D:\out
#
# Needs Rust and the .NET 8 SDK on the machine that builds it. The machine
# that runs it needs neither: office-host is published self-contained, so
# only Windows and a desktop Office are required there.
#
# v0.1.0 is Office only: the package carries the console, the CLI, the MCP
# server and the Office helper. The UI Automation helper and the browser
# hand are not in it; they stay in the source build for whoever turns them on.

param(
    [string]$Out = ''
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Out) { $Out = Join-Path $repo 'dist' }

# The version is the core crate's, so there is one place to bump it.
$version = (Select-String -Path (Join-Path $repo 'core\Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$name = "Syn-$version-win-x64"
$stage = Join-Path $Out $name
Write-Host "packaging $name"

if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

# --- the Rust side: console, CLI, MCP server ---------------------------------
Push-Location (Join-Path $repo 'core')
try {
    cargo build --release --bins
    if ($LASTEXITCODE) { throw "cargo build failed" }
} finally { Pop-Location }
foreach ($bin in 'ui', 'cli', 'mcpgate') {
    Copy-Item (Join-Path $repo "core\target\release\$bin.exe") $stage
}

# --- the Office helper, self-contained ----------------------------------------
# One file, carrying its own .NET, so the person running Syn installs nothing.
# Not trimmed: every Office call is late-bound `dynamic`, which trimming
# cannot see and would strip the binder out from under.
$pub = Join-Path $Out 'office-host-publish'
if (Test-Path $pub) { Remove-Item -Recurse -Force $pub }
dotnet publish (Join-Path $repo 'sidecar-csharp\Host\Host.csproj') -c Release -r win-x64 --self-contained true `
    -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true -p:PublishTrimmed=false `
    -p:Version=$version --nologo -o $pub
if ($LASTEXITCODE) { throw "dotnet publish failed" }
Copy-Item (Join-Path $pub 'office-host.exe') $stage
Remove-Item -Recurse -Force $pub

# --- what a person reads and double-clicks -----------------------------------
# A .env beside the programs: every one of them finds it by walking up from
# where it lives, so the chats, the saved keys and the settings all stay in
# this folder whoever starts them -- the launcher, a terminal, an MCP client.
@"
# Syn $version settings. Every line is optional: a key can be added in the
# console instead (the key icon in the sidebar), and the Office helpers use
# these pipe names when the lines are missing.

# A key for an OpenAI-compatible provider (OpenRouter by default).
AGENT_API_KEY=

# Set any of these to off to stop offering that application.
AGENT_PIPE_EXCEL=hand-excel
AGENT_PIPE_WORD=hand-word
AGENT_PIPE_PPT=hand-powerpoint
"@ | Set-Content -Encoding ascii (Join-Path $stage '.env')

# The launcher: the console, with its page opened. Closing the window that
# appears stops Syn and every helper it started.
@"
@echo off
cd /d "%~dp0"
title Syn
ui.exe --open
"@ | Set-Content -Encoding ascii (Join-Path $stage 'Syn.cmd')

@"
Syn $version for Windows -- Excel, Word and PowerPoint, driven by a model.

START
  1. Unzip this folder somewhere you own (Documents is fine; not Program
     Files, which Syn cannot write its chats into).
  2. Double-click Syn.cmd. A window opens and your browser shows the console.
  3. Add an API key: the key icon at the bottom of the sidebar.
  4. Ask for something: "open C:\...\budget.xlsx and total column C".
  Closing the Syn window stops Syn and anything it started. Your Office
  applications and documents stay open; Syn never closes what it did not
  open, and never saves unless asked.

NEEDS
  Windows 10 or 11, and desktop Microsoft Office (Microsoft 365 or a
  perpetual version). Nothing else: the Office helper carries its own .NET.

FROM AN MCP CLIENT (Claude Desktop, OpenCode, ...)
  Point the client at mcpgate.exe in this folder. No key is needed; the
  client's own model does the work. To keep it to certain folders, add
  AGENT_MCP_ROOTS=C:\Users\you\Documents to .env.

FILES
  ui.exe           the console (Syn.cmd starts it)
  cli.exe          the agent the console drives; also a terminal REPL
  mcpgate.exe      the MCP server
  office-host.exe  the Office helper: started by the others when needed
  .env             settings; chats and saved keys go in .agent\ beside it

More: https://github.com/azzindani/Syn (docs\setup-windows.md, docs\mcp.md)
"@ | Set-Content -Encoding ascii (Join-Path $stage 'START HERE.txt')

Copy-Item (Join-Path $repo 'LICENSE') $stage

# --- zip, and say what was made ----------------------------------------------
$zip = Join-Path $Out "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $stage -DestinationPath $zip
$hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
Write-Host ""
Write-Host "made   $zip"
Write-Host "sha256 $hash"
Get-ChildItem $stage | ForEach-Object { Write-Host ("       {0,-18} {1,10:N0} bytes" -f $_.Name, $_.Length) }
