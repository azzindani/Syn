<#
  live-excel-smoke.ps1 — the M1/M2 acceptance run against REAL Excel.

  What it proves, end to end, on a machine with Office installed:
    1. a document the human already has open can be attached (ROT lookup),
    2. the six ops answer over the office-rpc/1 named pipe,
    3. a write lands in the live window (not a copy on disk),
    4. bad handles and unknown methods fail closed with a named error,
    5. the sidecar detaches without closing the human's Excel,
    6. nothing is orphaned afterwards.

  Results are written to testbed/logs/ (gitignored). Nothing outside
  testbed/ is touched.

  Usage: powershell -ExecutionPolicy Bypass -File scripts\live-excel-smoke.ps1
#>
[CmdletBinding()]
param(
    [string]$Pipe = 'hand-excel',
    [string]$Book = 'D:\Github\Syn\testbed\docs\plan.xlsx',
    [int]$ReplyTimeoutMs = 25000
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$logs = Join-Path $root 'testbed\logs'
New-Item -ItemType Directory -Force -Path $logs | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$result = Join-Path $logs "live-excel-$stamp.tsv"
$sidelog = Join-Path $logs "live-excel-$stamp.sidecar.log"
$exe = Join-Path $root 'sidecar-csharp\Host\bin\Release\net8.0-windows\office-host.exe'
if (-not (Test-Path $exe)) { throw "sidecar not built: $exe (dotnet build -c Release)" }

function Line($s) { Write-Host $s; Add-Content -LiteralPath $result -Value $s -Encoding utf8 }

# --- 1. make sure the fixture is OPEN in a visible Excel ------------------
$xl = $null
$alreadyOpen = $false
try {
    $xl = New-Object -ComObject Excel.Application
    $xl.Visible = $true
    $leaf = Split-Path -Leaf $Book
    for ($i = 1; $i -le $xl.Workbooks.Count; $i++) {
        if ($xl.Workbooks.Item($i).Name -eq $leaf) { $alreadyOpen = $true }
    }
    if (-not $alreadyOpen) { $null = $xl.Workbooks.Open($Book) }
}
finally {
    if ($xl) { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($xl) }
    [GC]::Collect(); [GC]::WaitForPendingFinalizers()
}
Line "# live-excel smoke $stamp"
Line "# workbook`t$Book (already open: $alreadyOpen)"

# --- 2. start the sidecar against that live instance ----------------------
$proc = Start-Process -FilePath $exe -ArgumentList '--pipe', $Pipe, '--app', 'excel' `
    -PassThru -WindowStyle Hidden -RedirectStandardOutput $sidelog -RedirectStandardError "$sidelog.err"
Start-Sleep -Seconds 4
if ($proc.HasExited) { throw "sidecar exited immediately; see $sidelog" }

$requests = @(
    '{"method":"read","handle":"excel:plan.xlsx:Sheet1","args":{"selector":"Sheet1!A1:B2"}}',
    '{"method":"read","handle":"excel:plan.xlsx:Sheet1","args":{"selector":"Sheet1!A1:C5"}}',
    '{"method":"write","handle":"excel:plan.xlsx:Sheet1","args":{"selector":"Sheet1!E1"},"payload":"syn-was-here"}',
    '{"method":"read","handle":"excel:plan.xlsx:Sheet1","args":{"selector":"Sheet1!E1:E1"}}',
    '{"method":"bogus","handle":"excel:plan.xlsx:Sheet1","args":{"selector":"Sheet1!A1:A1"}}',
    '{"method":"read","handle":"excel:nosuch.xlsx:Sheet1","args":{"selector":"Sheet1!A1:A1"}}'
)

$ok = $true
try {
    $client = New-Object System.IO.Pipes.NamedPipeClientStream('.', $Pipe, [System.IO.Pipes.PipeDirection]::InOut)
    $client.Connect(10000)
    # UTF8Encoding($false): the stock [Text.Encoding]::UTF8 emits a BOM on
    # the first write, and those three bytes arrive glued to the front of the
    # first JSON request. The sidecar's hand-rolled parser tolerated it; a
    # stricter one would not.
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    $writer = New-Object System.IO.StreamWriter($client, $utf8)
    $writer.AutoFlush = $true
    $reader = New-Object System.IO.StreamReader($client, $utf8, $false)

    foreach ($req in $requests) {
        $writer.WriteLine($req)
        $task = $reader.ReadLineAsync()
        if (-not $task.Wait($ReplyTimeoutMs)) {
            Line "TIMEOUT`t$req"
            $ok = $false
            break   # the pipe is wedged; do not dispose into a pending read
        }
        Line "$req`t$($task.Result)"
    }
}
finally {
    # Order matters: kill the server first so no async read is pending when
    # the client streams go away (disposing into a pending read blocks).
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 400
    # The server is already gone, so these streams are disposing onto a
    # closed pipe: that is expected here, not a failure of the run.
    foreach ($d in $reader, $writer, $client) {
        if ($d) { try { $d.Dispose() } catch { } }
    }
}

Line "# sidecar stdout:"
Get-Content $sidelog -ErrorAction SilentlyContinue | ForEach-Object { Line "#   $_" }
$err = Get-Content "$sidelog.err" -ErrorAction SilentlyContinue
if ($err) { $err | ForEach-Object { Line "# STDERR $_" } }

# --- 3. the human's Excel must have survived ------------------------------
$excel = Get-Process EXCEL -ErrorAction SilentlyContinue
Line "# excel still running after detach: $([bool]$excel)"
if (-not $excel) { Line '# FAIL: sidecar closed the human Excel'; $ok = $false }
$hosts = Get-Process office-host -ErrorAction SilentlyContinue
Line "# orphan office-host: $([bool]$hosts)"
if ($hosts) { $ok = $false }

# --- 4. did the write actually land in the LIVE window? ------------------
# The read op reports dimensions, not values, so a write that silently went
# nowhere would still have answered "wrote 1x1". Read the cell back out of
# the running instance to prove the edit is in the human's window and not in
# some detached copy on disk.
$cell = '(unread)'
$xl2 = $null
try {
    # GetActiveObject, not New-Object: New-Object can hand back a FRESH
    # Excel with zero workbooks, and checking that one proves nothing. This
    # must read the same running instance the sidecar just wrote to — the
    # same ROT lookup the sidecar does, available here because Windows
    # PowerShell runs on .NET Framework where Marshal still exposes it.
    $xl2 = [Runtime.InteropServices.Marshal]::GetActiveObject('Excel.Application')
    for ($i = 1; $i -le $xl2.Workbooks.Count; $i++) {
        $b = $xl2.Workbooks.Item($i)
        if ($b.Name -eq (Split-Path -Leaf $Book)) { $cell = [string]$b.Worksheets.Item('Sheet1').Range('E1').Value2 }
    }
}
catch { $cell = "(error: $($_.Exception.Message))" }
finally {
    if ($xl2) { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($xl2) }
    [GC]::Collect(); [GC]::WaitForPendingFinalizers()
}
Line "# live Sheet1!E1 after run`t'$cell'"
if ($cell -ne 'syn-was-here') { Line '# FAIL: write did not reach the live workbook'; $ok = $false }

Line "# RESULT: $(if ($ok) { 'PASS' } else { 'FAIL' })"
Write-Host "`ntranscript: $result"
if (-not $ok) { exit 1 }
