<#
  live-helper-exit.ps1 -- what Excel does when Syn ends, on real Office.

  Syn's Office helper (office-host) is stopped when the program that
  started it ends. It used to be terminated outright, and Excel then went
  on counting it as a client: a plain Quit() left an invisible EXCEL.EXE
  running for more than twelve minutes, which a double-clicked workbook
  could open into without a window. Now the helper is asked to stop first
  (desk.rs ask_to_stop), lets go of Office, and exits; and if Syn is ended
  outright it notices (--parent) and does the same.

  Three cases, each through the real mcpgate.exe:
    1. Syn starts Excel, opens a workbook, closes it, ends. Excel was
       Syn's own and has nothing open, so it must go by itself.
    2. Syn starts Excel, opens a workbook, ends. Excel must stay, showing
       the workbook, and a plain Quit() afterwards must really end it.
    3. As 1, but mcpgate is killed instead of ending: the helper must go
       within seconds, and Excel with it.

  Only a copy under testbed\docs is opened (new-testbed-docs.ps1 makes
  them). Excel must not be running when this starts: a person's own Excel
  would be taken up by Syn, and stay, which is right but proves nothing.
  The exit code is the number of failures.

  Build first:
    cd core; cargo build --release; cd ..
    dotnet build -c Release sidecar-csharp\Host

  Usage:
    powershell -ExecutionPolicy Bypass -File scripts\live-helper-exit.ps1
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$docs = Join-Path $root 'testbed\docs'
$plan = Join-Path $docs 'plan.xlsx'

$gate = @('core\target\release\mcpgate.exe', 'core\target\debug\mcpgate.exe') |
    ForEach-Object { Join-Path $root $_ } | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $gate) { throw "mcpgate.exe not found: cd core; cargo build --release" }
$officeHost = Join-Path $root 'sidecar-csharp\Host\bin\Release\net8.0-windows\office-host.exe'
if (-not (Test-Path $officeHost)) { throw "office-host.exe not found: dotnet build -c Release sidecar-csharp\Host" }
if (-not (Test-Path $plan)) { throw "missing $plan -- run scripts\new-testbed-docs.ps1 first" }
if (Get-Process EXCEL -ErrorAction SilentlyContinue) { throw "Excel is running: close it first, so that the Excel under test is one Syn started" }

$script:failures = 0
function Verdict([string]$label, [bool]$ok, [string]$why) {
    if (-not $ok) { $script:failures++ }
    Write-Host ("{0}  {1,-52} {2}" -f $(if ($ok) { 'PASS' } else { 'FAIL' }), $label, $why) -ForegroundColor $(if ($ok) { 'Green' } else { 'Red' })
}

# Waits for EXCEL.EXE to be gone, up to $seconds. Returns the seconds it took, or -1.
function Wait-ExcelGone([int]$seconds) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    while ($clock.Elapsed.TotalSeconds -lt $seconds) {
        if (-not (Get-Process EXCEL -ErrorAction SilentlyContinue)) { return $clock.Elapsed.TotalSeconds }
        Start-Sleep -Milliseconds 250
    }
    return -1
}

# One mcpgate on pipes of its own, so this run never lands on a helper
# another client holds. Returns the process; $script:id counts its requests.
function Start-Gate {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $gate
    $psi.WorkingDirectory = $root
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.CreateNoWindow = $true
    $psi.EnvironmentVariables['AGENT_PIPE_EXCEL'] = 'exit-excel'
    $psi.EnvironmentVariables['AGENT_PIPE_WORD'] = 'exit-word'
    $psi.EnvironmentVariables['AGENT_PIPE_PPT'] = 'exit-ppt'
    $psi.EnvironmentVariables['AGENT_OFFICE_HOST'] = $officeHost
    $psi.EnvironmentVariables['AGENT_MCP_ROOTS'] = $docs
    $script:id = 0
    $p = [System.Diagnostics.Process]::Start($psi)
    $null = Invoke-Rpc $p 'initialize' @{ protocolVersion = '2025-11-25'; capabilities = @{}; clientInfo = @{ name = 'live-helper-exit'; version = '1' } }
    return $p
}

function Invoke-Rpc($p, [string]$method, $params) {
    $script:id++
    $msg = @{ jsonrpc = '2.0'; id = $script:id; method = $method; params = $params } | ConvertTo-Json -Compress -Depth 10
    $p.StandardInput.WriteLine($msg)
    $p.StandardInput.Flush()
    $line = $p.StandardOutput.ReadLine()
    if ($null -eq $line) { throw "mcpgate closed its output" }
    return $line | ConvertFrom-Json
}

function Call($p, [string]$tool, $arguments) {
    $r = Invoke-Rpc $p 'tools/call' @{ name = $tool; arguments = $arguments }
    if (-not $r.result -or $r.result.isError) { throw "$tool failed: $($r.result.content[0].text)" }
    return [string]$r.result.content[0].text
}

# Ending the client is what ends Syn's server, and the server ends its helper.
function Stop-Gate($p) {
    $p.StandardInput.Close()
    if (-not $p.WaitForExit(30000)) { $p.Kill() }
}

# --- 1. open, close, end: Excel was Syn's own and has nothing open --------
$p = Start-Gate
$opened = Call $p 'open' @{ app = 'excel'; path = $plan }
$handle = [regex]::Match($opened, 'Handle: (\S+)').Groups[1].Value
if (-not $handle) { throw "no handle in: $opened" }
$null = Call $p 'struct' @{ handle = $handle; verb = 'close' }
Stop-Gate $p
$took = Wait-ExcelGone 20
Verdict 'Excel Syn started goes when Syn ends' ($took -ge 0) $(if ($took -ge 0) { "gone {0:N1} s after Syn ended" -f $took } else { 'EXCEL.EXE still running 20 s after Syn ended' })
if (Get-Process EXCEL -ErrorAction SilentlyContinue) { Get-Process EXCEL | Stop-Process -Force; Start-Sleep -Seconds 2 }
Verdict 'no office-host is left' (-not (Get-Process office-host -ErrorAction SilentlyContinue)) ''

# --- 2. open, end: the workbook is still there to look at -----------------
$p = Start-Gate
$null = Call $p 'open' @{ app = 'excel'; path = $plan }
Stop-Gate $p
Start-Sleep -Seconds 3
$excel = Get-Process EXCEL -ErrorAction SilentlyContinue | Select-Object -First 1
Verdict 'Excel stays, showing the workbook, when Syn ends' ($excel -and $excel.MainWindowTitle -like '*plan.xlsx*') $(if ($excel) { "window: '$($excel.MainWindowTitle)'" } else { 'EXCEL.EXE is gone' })

# A person closing the workbook and Excel: a plain Quit() must now end it.
if ($excel) {
    $xl = [Runtime.InteropServices.Marshal]::GetActiveObject('Excel.Application')
    $safe = $true
    foreach ($b in @($xl.Workbooks)) { if (-not ([string]$b.FullName).StartsWith($docs, [StringComparison]::OrdinalIgnoreCase)) { $safe = $false } }
    if ($safe) {
        $xl.DisplayAlerts = $false
        foreach ($b in @($xl.Workbooks)) { $b.Saved = $true }
        $xl.Quit()
    }
    $b = $null
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($xl)
    $xl = $null
    [GC]::Collect(); [GC]::WaitForPendingFinalizers()
    if (-not $safe) { Verdict 'a plain Quit() ends Excel afterwards' $false 'Excel holds a workbook that is not a testbed copy: left alone' }
    else {
        $took = Wait-ExcelGone 40
        Verdict 'a plain Quit() ends Excel afterwards' ($took -ge 0) $(if ($took -ge 0) { "gone {0:N1} s after Quit()" -f $took } else { 'EXCEL.EXE still running 40 s after Quit()' })
    }
}
# --- 3. Syn ended outright: nobody gets to ask the helper anything ---------
# Task Manager, a crash, a client that kills its server. The helper notices
# its launcher has gone and lets go of Office by itself.
$p = Start-Gate
$opened = Call $p 'open' @{ app = 'excel'; path = $plan }
$handle = [regex]::Match($opened, 'Handle: (\S+)').Groups[1].Value
$null = Call $p 'struct' @{ handle = $handle; verb = 'close' }
$p.Kill()
$p.WaitForExit()
$clock = [Diagnostics.Stopwatch]::StartNew()
while ((Get-Process office-host -ErrorAction SilentlyContinue) -and $clock.Elapsed.TotalSeconds -lt 20) { Start-Sleep -Milliseconds 250 }
$hostGone = -not (Get-Process office-host -ErrorAction SilentlyContinue)
Verdict 'the helper goes when Syn is killed outright' $hostGone $(if ($hostGone) { "gone {0:N1} s after the kill" -f $clock.Elapsed.TotalSeconds } else { 'office-host.exe still running 20 s after the kill' })
$took = Wait-ExcelGone 20
Verdict 'Excel Syn started goes too, with nothing open' ($took -ge 0) $(if ($took -ge 0) { "gone {0:N1} s after the kill" -f $took } else { 'EXCEL.EXE still running 20 s after the kill' })
if (Get-Process EXCEL -ErrorAction SilentlyContinue) { Get-Process EXCEL | Stop-Process -Force; Start-Sleep -Seconds 2 }
if (Get-Process office-host -ErrorAction SilentlyContinue) { Verdict 'no office-host is left' $false 'office-host.exe is still running' }

Write-Host ''
Write-Host ("{0} failure(s)." -f $script:failures) -ForegroundColor $(if ($script:failures) { 'Yellow' } else { 'Green' })
exit $script:failures
