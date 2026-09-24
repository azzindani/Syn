# live-cdp-smoke.ps1 - drive a real Chromium through the CDP hand.
#
# Starts a throwaway browser on a debugging port, runs the six ops against a
# live page through the CLI, then reaps the whole process tree and proves the
# port is closed. Everything it writes lives under testbed/, which is
# gitignored.
#
# The browser gets its own --user-data-dir on purpose. Chrome refuses
# --remote-debugging-port for a profile that is already running, so pointing
# this at your everyday profile would either fail or, worse, expose your
# logged-in sessions on an unauthenticated local port.
#
#   pwsh -File scripts/live-cdp-smoke.ps1 [-Port 9222] [-Browser chrome|edge]

param(
    [int]$Port = 9222,
    [ValidateSet('chrome', 'edge')][string]$Browser = 'chrome'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$bed = Join-Path $repo 'testbed'
$cli = Join-Path $repo 'core\target\debug\cli.exe'

if (-not (Test-Path $cli)) { throw "build the cli first: cd core; cargo build --bins" }
New-Item -ItemType Directory -Force -Path $bed | Out-Null

# Preflight: never fight for a port something else already owns.
if (netstat -ano | Select-String ":$Port\s" | Select-String 'LISTENING') {
    throw "port $Port is already in use. Close it or pass -Port."
}

$exe = if ($Browser -eq 'edge') {
    'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe'
} else {
    'C:\Program Files\Google\Chrome\Application\chrome.exe'
}
if (-not (Test-Path $exe)) { throw "$Browser not found at $exe" }

$page = Join-Path $bed 'cdp-page.html'
@'
<!doctype html>
<html><head><title>Syn CDP testbed</title></head>
<body><main id="report"><h1>Quarterly plan</h1><p id="status">not started</p></main></body></html>
'@ | Set-Content -Path $page -Encoding ascii

$profileDir = Join-Path $bed "cdp-profile-$Port"
$proc = Start-Process -FilePath $exe -PassThru -ArgumentList `
    "--remote-debugging-port=$Port", "--user-data-dir=$profileDir", `
    '--no-first-run', '--no-default-browser-check', "file:///$($page -replace '\\','/')"
Write-Host "started $Browser pid=$($proc.Id) port=$Port"

try {
    # Wait for the port rather than sleeping a guessed interval.
    $ready = $false
    foreach ($i in 1..20) {
        Start-Sleep -Milliseconds 500
        try {
            Invoke-WebRequest -Uri "http://127.0.0.1:$Port/json/version" -UseBasicParsing -TimeoutSec 2 | Out-Null
            $ready = $true
            break
        } catch {}
    }
    if (-not $ready) { throw "devtools port $Port never came up" }

    $script = @"
cdp 127.0.0.1:$Port web
page web testbed #report
live web:testbed:#report
read web:testbed:#report h1
write web:testbed:#report #status shipped-by-syn
read web:testbed:#report #status
format web:testbed:#report #status color=crimson
para web:testbed:#report Added live by Syn.
export web:testbed:#report html
undo web:testbed:#report
allow nothing
read web:testbed:#report h1
quit
"@
    $inFile = Join-Path $bed "cdp-smoke-$Port.txt"
    # No BOM: a byte-order mark at the head of the pipe becomes part of the
    # first command. Set-Content -Encoding utf8 writes one on PS 5.1.
    [System.IO.File]::WriteAllText($inFile, $script, (New-Object System.Text.UTF8Encoding($false)))
    Get-Content $inFile | & $cli 2>&1 | Tee-Object -FilePath (Join-Path $bed "cdp-smoke-$Port.log")
} finally {
    # Reap the TREE. Killing the launcher alone leaves renderers holding the
    # port, which is how this project previously stranded a process for an
    # hour.
    & taskkill /T /F /PID $proc.Id 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $left = Get-CimInstance Win32_Process -Filter "Name='chrome.exe' OR Name='msedge.exe'" |
        Where-Object { $_.CommandLine -like "*$profileDir*" }
    if ($left) {
        Write-Warning "LEFTOVER: $($left.ProcessId -join ', ')"
    } else {
        Write-Host 'clean: no testbed browser processes remain'
    }
    if (netstat -ano | Select-String ":$Port\s" | Select-String 'LISTENING') {
        Write-Warning "port $Port still listening"
    } else {
        Write-Host "clean: port $Port closed"
    }
}
