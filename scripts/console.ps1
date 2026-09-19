# console.ps1 - start the Syn web console and open it.
#
# The console is a local page that types commands into the real CLI. It binds
# 127.0.0.1 only, and a command POST is refused unless its Origin is the
# console's own, so nothing off this machine can reach it and no web page you
# happen to visit can post to it.
#
#   powershell -File scripts/console.ps1              # console only
#   powershell -File scripts/console.ps1 -WithUia     # also start the UIA hand
#   powershell -File scripts/console.ps1 -Port 7788
#   powershell -File scripts/console.ps1 -NoOpen   # print the URL, open it yourself
#
# Stop it with:  Get-Process ui, uia-host | Stop-Process

param(
    [int]$Port = 7777,
    [switch]$WithUia,
    [switch]$NoOpen,
    [string]$Pipe = 'synuia'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$bed = Join-Path $repo 'testbed'
$ui = Join-Path $repo 'core\target\debug\ui.exe'
$uiaExe = Join-Path $repo 'sidecar-csharp\Uia\bin\Release\net8.0-windows\uia-host.exe'

if (-not (Test-Path $ui)) { throw "build it first: cd core; cargo build --bins" }
New-Item -ItemType Directory -Force -Path $bed | Out-Null

# One console at a time, or the second fails to bind and the first is the one
# you are actually talking to.
Get-Process ui -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

if ($WithUia) {
    if (-not (Test-Path $uiaExe)) {
        throw "build the sidecar first: dotnet build -c Release sidecar-csharp\Uia\Uia.csproj"
    }
    Get-Process uia-host -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    # No -Redirect*: redirection forces UseShellExecute=false, and CreateProcess
    # then inherits this shell's handles into a process that outlives it. The
    # server holds the launcher's stdout open forever, so anything reading the
    # launcher hangs. Run the sidecar by hand with --trace if you want its log.
    $uia = Start-Process -FilePath $uiaExe -ArgumentList '--pipe', $Pipe -PassThru -WindowStyle Hidden
    Write-Host "uia-host pid=$($uia.Id) pipe=$Pipe  (click 'UI Automation' in the console to attach)"
}

# Same reason: no redirection. The console writes its address to --url-file
# instead, which is why that option exists.
$urlFile = Join-Path $bed 'console-url.txt'
Remove-Item $urlFile -ErrorAction SilentlyContinue
$proc = Start-Process -FilePath $ui -ArgumentList '--port', $Port, '--url-file', $urlFile -PassThru -WindowStyle Hidden

# Read the address from the server rather than guessing at the port.
$url = $null
foreach ($i in 1..25) {
    Start-Sleep -Milliseconds 400
    if ($proc.HasExited) { throw "ui.exe exited: is port $Port already in use?" }
    if (Test-Path $urlFile) {
        $first = Get-Content $urlFile -TotalCount 1 -ErrorAction SilentlyContinue
        if ($first -match 'http://\S+') { $url = $Matches[0]; break }
    }
}
if (-not $url) { throw "the console never wrote its address to $urlFile" }

Write-Host "console pid=$($proc.Id)"
Write-Host $url
# Plain Start-Process: with no redirection this goes through ShellExecute,
# which does not inherit our handles. Pass -NoOpen to skip it.
if (-not $NoOpen) { Start-Process -FilePath $url | Out-Null }
Write-Host ''
Write-Host 'Stop everything with:  Get-Process ui, uia-host -ErrorAction SilentlyContinue | Stop-Process'
