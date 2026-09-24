# live-uia-smoke.ps1 - drive a real Windows app through the UIA hand.
#
# Starts Calculator and the uia-host sidecar, reads the control tree, presses
# buttons by identity (not by coordinate), reads the result back, and proves
# a bad selector is refused. Then it reaps everything it started.
#
# Calculator is the target on purpose: it has no automation API, no plugin
# and no scripting surface, and it is the app these demos are usually done on
# with screenshots and mouse coordinates. Everything below is structural.
#
#   powershell -File scripts/live-uia-smoke.ps1 [-Pipe hand-uia]

param([string]$Pipe = 'hand-uia')

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$bed = Join-Path $repo 'testbed'
$cli = Join-Path $repo 'core\target\debug\cli.exe'
$host_ = Join-Path $repo 'sidecar-csharp\Uia\bin\Release\net8.0-windows\uia-host.exe'

if (-not (Test-Path $cli)) { throw "build the cli first: cd core; cargo build --bins" }
if (-not (Test-Path $host_)) { throw "build the sidecar first: dotnet build -c Release sidecar-csharp\Uia\Uia.csproj" }
New-Item -ItemType Directory -Force -Path $bed | Out-Null

$calc = $null
$uia = $null
# calc.exe is a stub that launches CalculatorApp.exe as a SIBLING, not a
# child, so taskkill /T against the stub never reaches the real window. Note
# which ones were already yours so the cleanup kills only what this script
# started.
$preexisting = @(Get-Process CalculatorApp -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
try {
    $calc = Start-Process -FilePath 'calc.exe' -PassThru
    $uia = Start-Process -FilePath $host_ -ArgumentList '--pipe', $Pipe -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $bed 'uia-host.log') `
        -RedirectStandardError (Join-Path $bed 'uia-host.err')
    Start-Sleep -Seconds 4
    if ($uia.HasExited) { throw "uia-host exited immediately; see testbed\uia-host.err" }
    Write-Host "calc pid=$($calc.Id) uia-host pid=$($uia.Id) pipe=$Pipe"

    $script = @"
hand $Pipe ui
hands
win ui Calculator :self
live ui:Calculator::self
read ui:Calculator::self :tree
read ui:Calculator::self id=CalculatorResults
invoke ui:Calculator::self id=num7Button
invoke ui:Calculator::self id=plusButton
invoke ui:Calculator::self id=num5Button
invoke ui:Calculator::self id=equalButton
read ui:Calculator::self id=CalculatorResults
invoke ui:Calculator::self id=nosuchButton
read ui:Calculator::self id=CalculatorResults
allow word
read ui:Calculator::self id=CalculatorResults
quit
"@
    $inFile = Join-Path $bed 'uia-smoke.txt'
    # No BOM: it would become part of the first command.
    [System.IO.File]::WriteAllText($inFile, $script, (New-Object System.Text.UTF8Encoding($false)))
    Get-Content $inFile | & $cli 2>&1 | Tee-Object -FilePath (Join-Path $bed 'uia-smoke.log')

    if ($uia.HasExited) {
        Write-Warning 'uia-host died during the session: it must survive a client disconnecting'
    } else {
        Write-Host 'sidecar still up: it serves the next session too'
    }
} finally {
    foreach ($p in @($uia, $calc)) {
        if ($p -and -not $p.HasExited) { & taskkill /T /F /PID $p.Id 2>&1 | Out-Null }
    }
    Get-Process CalculatorApp -ErrorAction SilentlyContinue |
        Where-Object { $preexisting -notcontains $_.Id } |
        ForEach-Object { & taskkill /T /F /PID $_.Id 2>&1 | Out-Null }
    Start-Sleep -Seconds 2
    $left = @(Get-Process uia-host -ErrorAction SilentlyContinue) +
            @(Get-Process CalculatorApp -ErrorAction SilentlyContinue |
                Where-Object { $preexisting -notcontains $_.Id })
    if ($left.Count -gt 0) {
        Write-Warning "LEFTOVER: $(($left | ForEach-Object { "$($_.ProcessName):$($_.Id)" }) -join ', ')"
    } else {
        Write-Host 'clean: nothing this script started is still running'
    }
}
