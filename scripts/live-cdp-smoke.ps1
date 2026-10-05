# live-cdp-smoke.ps1 - drive Syn's own browser, for real, and watch it.
#
# Runs the two live browser suites against a real Chrome or Edge with its
# window showing, through the real mcpgate:
#
#   tests/browser/test_browser_live.py  Syn starts a browser of its own on a
#       profile of its own, drives a fixture site (forms, frames, dialogs,
#       menus, new tabs, secrets), and must leave no browser process behind.
#   tests/test_cdp_live.py              the other mode: a browser a person
#       started and pointed Syn at (AGENT_CDP), which Syn must never close.
#
# Both start a throwaway profile and a port the system picks, so nothing of
# yours is touched: the browser they open is not your Chrome, and your
# sign-ins are not in it. A failure prints which step and why; the exit code
# is the number of failed suites.
#
#   pwsh -File scripts/live-cdp-smoke.ps1 [-Browser chrome|edge] [-Headless]

param(
    [ValidateSet('chrome', 'edge')][string]$Browser = 'chrome',
    [switch]$Headless
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$gate = Join-Path $repo 'core\target\debug\mcpgate.exe'
if (-not (Test-Path $gate)) { throw "build first: cd core; cargo build --bins" }

$exe = if ($Browser -eq 'edge') {
    'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe'
} else {
    'C:\Program Files\Google\Chrome\Application\chrome.exe'
}
if (-not (Test-Path $exe)) { throw "$Browser not found at $exe" }

$python = (Get-Command python -ErrorAction SilentlyContinue)
if (-not $python) { $python = (Get-Command py -ErrorAction SilentlyContinue) }
if (-not $python) { throw "Python 3 is needed to run the fixture site (python.org, or the Microsoft Store)" }

$env:AGENT_BROWSER = $exe
$env:SYN_BROWSER = $exe
$env:SYN_REQUIRE_BROWSER = '1'
if (-not $Headless) { $env:SYN_BROWSER_VISIBLE = '1' }

$failed = 0
foreach ($suite in 'tests\browser\test_browser_live.py', 'tests\test_cdp_live.py') {
    Write-Host "== $suite ($Browser$(if ($Headless) { ', no window' }))"
    & $python.Source -W ignore (Join-Path $repo $suite)
    if ($LASTEXITCODE -ne 0) {
        $failed++
        Write-Warning "$suite failed"
    }
}

# The suites check their own profiles; this checks that nothing at all of
# Syn's is still running, the way a person would notice it in Task Manager.
Start-Sleep -Seconds 2
$left = Get-CimInstance Win32_Process |
    Where-Object { $_.Name -match '^(chrome|msedge)\.exe$' -and $_.CommandLine -like '*syn-*' }
if ($left) {
    Write-Warning "LEFTOVER browser processes on a test profile: $($left.ProcessId -join ', ')"
    $failed++
} else {
    Write-Host 'clean: no test browser processes remain'
}
exit $failed
