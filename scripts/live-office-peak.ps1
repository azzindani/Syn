<#
  live-office-peak.ps1 -- every Office verb, once, against real Office.

  Drives the real mcpgate.exe over stdio exactly as an MCP client does, so
  what this proves is the whole road a model's call takes: MCP -> the gates
  -> the named pipe -> office-host -> COM -> Excel, Word, PowerPoint. Each
  step prints PASS or FAIL with what came back, and the exit code is the
  number of failures, so the output pasted back is the whole report.

  It works on throwaway documents that new-testbed-docs.ps1 makes in
  testbed\docs, never on yours, and it closes nothing: when it finishes,
  Excel, Word and PowerPoint are left open on those documents so you can
  look at what each step did. Exports land in testbed\out\peak.*.

  Build first:
    cd core; cargo build --release; cd ..
    dotnet build -c Release sidecar-csharp\Host

  Usage:
    powershell -ExecutionPolicy Bypass -File scripts\live-office-peak.ps1
      -SkipDocs   reuse testbed\docs as they are (default: make them fresh,
                  which needs Office closed -- see new-testbed-docs.ps1)
      -Vba        also write, run and read back a VBA macro. Needs Trust
                  Center > Macro Settings > Trust access to the VBA project
                  object model, and sets AGENT_VBA=1 for this run only
      -Theme <path to a .thmx or .potx>   also apply a theme to the deck
#>
[CmdletBinding()]
param([switch]$SkipDocs, [switch]$Vba, [string]$Theme = '')

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$docs = Join-Path $root 'testbed\docs'
$out = Join-Path $root 'testbed\out'
New-Item -ItemType Directory -Force -Path $out | Out-Null

# --- what has to be built ---------------------------------------------------
$gate = @('core\target\release\mcpgate.exe', 'core\target\debug\mcpgate.exe') |
    ForEach-Object { Join-Path $root $_ } | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $gate) { throw "mcpgate.exe not found: cd core; cargo build --release" }
$officeHost = Join-Path $root 'sidecar-csharp\Host\bin\Release\net8.0-windows\office-host.exe'
if (-not (Test-Path $officeHost)) { throw "office-host.exe not found: dotnet build -c Release sidecar-csharp\Host" }

if (-not $SkipDocs) {
    & (Join-Path $PSScriptRoot 'new-testbed-docs.ps1')
    if ($LASTEXITCODE -ne 0 -and $null -ne $LASTEXITCODE) { throw "new-testbed-docs.ps1 failed" }
}
foreach ($n in 'plan.xlsx', 'report.docx', 'deck.pptx') {
    if (-not (Test-Path (Join-Path $docs $n))) { throw "missing testbed\docs\$($n) -- run without -SkipDocs" }
}

# --- the MCP server -----------------------------------------------------------
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $gate
$psi.WorkingDirectory = $root
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $false
$psi.CreateNoWindow = $true
# Pipes of its own, so this run never lands on a helper another client holds.
$psi.EnvironmentVariables['AGENT_PIPE_EXCEL'] = 'peak-excel'
$psi.EnvironmentVariables['AGENT_PIPE_WORD'] = 'peak-word'
$psi.EnvironmentVariables['AGENT_PIPE_PPT'] = 'peak-ppt'
$psi.EnvironmentVariables['AGENT_OFFICE_HOST'] = $officeHost
$psi.EnvironmentVariables['AGENT_MCP_ROOTS'] = "$docs;$out"
if ($Vba) { $psi.EnvironmentVariables['AGENT_VBA'] = '1' }
$gateProc = [System.Diagnostics.Process]::Start($psi)
$script:id = 0

function Invoke-Rpc([string]$method, $params) {
    $script:id++
    $msg = @{ jsonrpc = '2.0'; id = $script:id; method = $method; params = $params } | ConvertTo-Json -Compress -Depth 10
    $gateProc.StandardInput.WriteLine($msg)
    $gateProc.StandardInput.Flush()
    $line = $gateProc.StandardOutput.ReadLine()
    if ($null -eq $line) { throw "mcpgate closed its output" }
    return $line | ConvertFrom-Json
}

$null = Invoke-Rpc 'initialize' @{ protocolVersion = '2025-11-25'; capabilities = @{}; clientInfo = @{ name = 'live-office-peak'; version = '1' } }

$script:results = New-Object System.Collections.ArrayList

# One call, one line of report. -Expect is a regex the reply must match;
# -Fails means the call is meant to be refused.
function Step([string]$label, [string]$tool, [hashtable]$arguments, [string]$Expect = '', [switch]$Fails) {
    $text = ''
    $isError = $true
    try {
        $r = Invoke-Rpc 'tools/call' @{ name = $tool; arguments = $arguments }
        if ($r.result) {
            $isError = [bool]$r.result.isError
            $text = [string]$r.result.content[0].text
        } else {
            $text = "protocol error: $($r.error.message)"
        }
    } catch {
        $text = "script error: $($_.Exception.Message)"
    }
    $ok = ($isError -eq [bool]$Fails) -and ($Expect -eq '' -or $text -match $Expect)
    $flat = ($text -replace '\s+', ' ').Trim()
    if ($flat.Length -gt 170) { $flat = $flat.Substring(0, 170) + '...' }
    $tag = if ($ok) { 'PASS' } else { 'FAIL' }
    $colour = if ($ok) { 'Green' } else { 'Red' }
    Write-Host ("{0}  {1,-44} {2}" -f $tag, $label, $flat) -ForegroundColor $colour
    [void]$script:results.Add([pscustomobject]@{ Ok = $ok; Label = $label; Text = $text })
    return $text
}

function Skip([string]$label, [string]$why) {
    Write-Host ("SKIP  {0,-44} {1}" -f $label, $why) -ForegroundColor Yellow
}

$x = 'excel:plan.xlsx:workbook'
$w = 'word:report.docx:body'
$p = 'ppt:deck.pptx:deck'

# ============================================================== Excel
Write-Host "`n-- Excel --" -ForegroundColor Cyan
Step 'open plan.xlsx' 'open' @{ app = 'excel'; path = (Join-Path $docs 'plan.xlsx') } -Expect 'Sheet1' | Out-Null
Step 'find West' 'struct' @{ handle = $x; verb = 'find'; selector = 'Sheet1'; text = 'West' } -Expect 'Sheet1!A5' | Out-Null
Step 'insert row 3' 'struct' @{ handle = $x; verb = 'insert'; selector = 'Sheet1!3:3' } | Out-Null
Step '  row 3 is empty, South moved to 4' 'read' @{ handle = $x; selector = 'Sheet1!A3:A4' } -Expect '= ;South' | Out-Null
Step 'delete row 3' 'struct' @{ handle = $x; verb = 'delete'; selector = 'Sheet1!3:3' } | Out-Null
Step '  South is back on row 3' 'read' @{ handle = $x; selector = 'Sheet1!A3' } -Expect '= South' | Out-Null
Step 'sort by Q3 Revenue, largest first' 'struct' @{ handle = $x; verb = 'sort'; selector = 'Sheet1!A1:C5'; name = 'Q3 Revenue'; rule = 'desc' } | Out-Null
Step '  West is first, header stayed' 'read' @{ handle = $x; selector = 'Sheet1!A1:A2' } -Expect '= Region;West' | Out-Null
Step 'filter Region = North' 'struct' @{ handle = $x; verb = 'filter'; selector = 'Sheet1!A1:C5'; name = 'Region'; rule = 'North' } -Expect '1 row' | Out-Null
Step 'clear the filter' 'struct' @{ handle = $x; verb = 'filter'; selector = 'Sheet1!A1:C5'; name = 'Region'; rule = '' } -Expect 'cleared' | Out-Null
Step 'write a duplicate row' 'write' @{ handle = $x; selector = 'Sheet1!A6:C6'; values = 'West|501200|0.19' } | Out-Null
Step 'dedupe' 'struct' @{ handle = $x; verb = 'dedupe'; selector = 'Sheet1!A1:C6' } -Expect '5 data row\(s\) before, 4 after' | Out-Null
Step 'addSheet Copy' 'struct' @{ handle = $x; verb = 'addSheet'; name = 'Copy' } | Out-Null
Step 'copy the table to Copy!B2' 'struct' @{ handle = $x; verb = 'copy'; source = 'Sheet1!A1:C5'; at = 'Copy!B2' } | Out-Null
Step '  it arrived' 'read' @{ handle = $x; selector = 'Copy!B2:D2' } -Expect 'Region\|Q3 Revenue\|Growth' | Out-Null
Step 'rename Copy to Copied' 'struct' @{ handle = $x; verb = 'sheet'; selector = 'Copy'; action = 'rename'; name = 'Copied' } | Out-Null
Step 'copy the sheet as Again' 'struct' @{ handle = $x; verb = 'sheet'; selector = 'Copied'; action = 'copy'; name = 'Again' } | Out-Null
Step 'hide Again' 'struct' @{ handle = $x; verb = 'sheet'; selector = 'Again'; action = 'hide' } | Out-Null
Step 'show Again' 'struct' @{ handle = $x; verb = 'sheet'; selector = 'Again'; action = 'show' } | Out-Null
Step 'delete Again' 'struct' @{ handle = $x; verb = 'sheet'; selector = 'Again'; action = 'delete' } | Out-Null
Step '  Again is gone' 'read' @{ handle = $x; selector = 'Again!A1' } -Fails -Expect 'no sheet named' | Out-Null
Step 'drop-down on D2:D5' 'struct' @{ handle = $x; verb = 'validate'; selector = 'Sheet1!D2:D5'; rule = 'list=Yes,No,Maybe' } | Out-Null
Step 'whole numbers 1..10 on E2:E5' 'struct' @{ handle = $x; verb = 'validate'; selector = 'Sheet1!E2:E5'; rule = 'whole=1..10' } | Out-Null
Step 'a two-line note on A1' 'struct' @{ handle = $x; verb = 'comment'; selector = 'Sheet1!A1'; text = "first line`nsecond line" } | Out-Null
Step 'a link on A8' 'struct' @{ handle = $x; verb = 'link'; selector = 'Sheet1!A8'; text = 'https://example.com'; title = 'Example' } | Out-Null
Step 'format keys: underline, valign, height' 'format' @{ handle = $x; selector = 'Sheet1!A1:C1'; style = 'underline=1;valign=center;height=24' } | Out-Null
Step 'hide row 5' 'format' @{ handle = $x; selector = 'Sheet1!5:5'; style = 'hidden=1' } | Out-Null
Step 'show row 5' 'format' @{ handle = $x; selector = 'Sheet1!5:5'; style = 'hidden=0' } | Out-Null
Step 'scatter chart' 'struct' @{ handle = $x; verb = 'chart'; kind = 'scatter'; source = 'Sheet1!B1:C5'; at = 'Copied!F2:M16'; title = 'Revenue vs growth' } | Out-Null
Step 'doughnut chart' 'struct' @{ handle = $x; verb = 'chart'; kind = 'doughnut'; source = 'Sheet1!A1:B5'; at = 'Copied!F18:M32'; title = 'Revenue share' } | Out-Null
Step 'export the charts as png' 'export' @{ handle = $x; format = 'png'; path = (Join-Path $out 'peak-charts.png') } | Out-Null
$png = Get-ChildItem $out -Filter 'peak-charts*.png' -ErrorAction SilentlyContinue | Select-Object -First 1
if ($png) {
    Step 'picture on a sheet' 'struct' @{ handle = $x; verb = 'picture'; selector = 'Copied!O2:V16'; text = $png.FullName } | Out-Null
} else { Skip 'picture on a sheet' 'no chart png was exported to place' }
Step 'page setup: landscape, one page wide' 'struct' @{ handle = $x; verb = 'pageSetup'; selector = 'Sheet1'; style = 'orientation=landscape;paper=A4;fitWide=1;fitTall=0' } | Out-Null
Step 'replace North with Nord' 'struct' @{ handle = $x; verb = 'replace'; selector = 'Sheet1'; text = 'North'; with = 'Nord' } -Expect 'in 1 cell' | Out-Null
if ($Vba) {
    $code = "Sub SynPeak()`r`n    Worksheets(""Sheet1"").Range(""H1"").Value = ""from VBA""`r`nEnd Sub"
    Step 'macro: write a multi-line module' 'struct' @{ handle = $x; verb = 'macro'; action = 'write'; name = 'SynPeak'; code = $code } | Out-Null
    Step 'macro: run it' 'struct' @{ handle = $x; verb = 'macro'; action = 'run'; title = 'SynPeak' } | Out-Null
    Step '  it wrote H1' 'read' @{ handle = $x; selector = 'Sheet1!H1' } -Expect 'from VBA' | Out-Null
} else { Skip 'macro write, run, read' 'pass -Vba to include it' }
Step 'export xlsx' 'export' @{ handle = $x; format = 'xlsx'; path = (Join-Path $out 'peak.xlsx') } | Out-Null

# =============================================================== Word
Write-Host "`n-- Word --" -ForegroundColor Cyan
Step 'open report.docx' 'open' @{ app = 'word'; path = (Join-Path $docs 'report.docx') } | Out-Null
Step 'insert a paragraph before p1' 'struct' @{ handle = $w; verb = 'insertParagraph'; text = 'Inserted before the body'; at = 'p1' } | Out-Null
Step '  it is p1' 'read' @{ handle = $w; selector = 'p1' } -Expect 'Inserted before the body' | Out-Null
Step '  the body moved to p2' 'read' @{ handle = $w; selector = 'p2' } -Expect 'Placeholder' | Out-Null
Step 'delete p1' 'struct' @{ handle = $w; verb = 'delete'; selector = 'p1' } | Out-Null
Step '  the body is p1 again' 'read' @{ handle = $w; selector = 'p1' } -Expect 'Placeholder' | Out-Null
Step 'a bullet (built-in style)' 'struct' @{ handle = $w; verb = 'insertParagraph'; name = 'List Bullet'; text = 'first point' } -Expect '\[List Bullet\]' | Out-Null
Step 'a numbered item (built-in style)' 'struct' @{ handle = $w; verb = 'insertParagraph'; name = 'List Number'; text = 'step one' } -Expect '\[List Number\]' | Out-Null
Step 'a table before p1' 'struct' @{ handle = $w; verb = 'insertTable'; rows = 'Site|Score;North|3;South|4'; at = 'p1' } | Out-Null
Step 'find Placeholder' 'struct' @{ handle = $w; verb = 'find'; text = 'Placeholder' } -Expect 'paragraph' | Out-Null
Step 'replace Placeholder with Draft' 'struct' @{ handle = $w; verb = 'replace'; text = 'Placeholder'; with = 'Draft' } -Expect '1 time' | Out-Null
Step 'a two-line comment on p0' 'struct' @{ handle = $w; verb = 'comment'; selector = 'p0'; text = "check this`nand this" } | Out-Null
Step 'link p0' 'struct' @{ handle = $w; verb = 'link'; selector = 'p0'; text = 'https://example.com' } | Out-Null
Step 'header' 'struct' @{ handle = $w; verb = 'header'; name = 'header'; text = 'Syn peak test' } | Out-Null
Step 'footer' 'struct' @{ handle = $w; verb = 'header'; name = 'footer'; text = 'Prepared by Syn' } | Out-Null
Step 'page numbers after the footer' 'struct' @{ handle = $w; verb = 'pageNumbers'; text = 'Page' } | Out-Null
Step 'page setup: landscape, 0.75in' 'struct' @{ handle = $w; verb = 'pageSetup'; style = 'orientation=landscape;margin=0.75' } | Out-Null
Step 'format keys on p0' 'format' @{ handle = $w; selector = 'p0'; style = 'underline=1;highlight=yellow;spaceAfter=12;lineSpacing=1.5' } | Out-Null
Step 'export docx' 'export' @{ handle = $w; format = 'docx'; path = (Join-Path $out 'peak.docx') } | Out-Null
Step 'export pdf' 'export' @{ handle = $w; format = 'pdf'; path = (Join-Path $out 'peak-report.pdf') } | Out-Null

# ========================================================= PowerPoint
Write-Host "`n-- PowerPoint --" -ForegroundColor Cyan
Step 'open deck.pptx' 'open' @{ app = 'powerpoint'; path = (Join-Path $docs 'deck.pptx') } | Out-Null
Step 'a slide with bullets' 'struct' @{ handle = $p; verb = 'createSlide'; title = 'Body'; bullets = 'one|two|>two a'; name = 'titleContent' } | Out-Null
Step 'rewrite its body' 'write' @{ handle = $p; selector = 's2.body'; values = 'alpha|beta|>beta one' } -Expect '3 bullet' | Out-Null
Step 'format the body' 'format' @{ handle = $p; selector = 's2.body'; style = 'size=20;align=left' } -Expect 'body' | Out-Null
Step 'a text box on s1' 'struct' @{ handle = $p; verb = 'textBox'; selector = 's1'; name = '60,300,500,40'; text = 'Source: Syn'; style = 'size=14;color=#1F4E79' } | Out-Null
Step 'duplicate s2' 'struct' @{ handle = $p; verb = 'duplicateSlide'; selector = 's2' } | Out-Null
Step '  three slides' 'read' @{ handle = $p; selector = 'deck' } -Expect 'slides=3' | Out-Null
Step 'move s3 to the front' 'struct' @{ handle = $p; verb = 'moveSlide'; selector = 's3'; at = 's1' } -Expect 'now s1' | Out-Null
Step 'find alpha' 'struct' @{ handle = $p; verb = 'find'; text = 'alpha' } -Expect 's1' | Out-Null
Step 'replace beta with gamma' 'struct' @{ handle = $p; verb = 'replace'; text = 'beta'; with = 'gamma' } -Expect 'time' | Out-Null
Step 'delete s1' 'struct' @{ handle = $p; verb = 'delete'; selector = 's1' } | Out-Null
Step '  two slides' 'read' @{ handle = $p; selector = 'deck' } -Expect 'slides=2' | Out-Null
if ($Theme) {
    Step 'apply a theme' 'struct' @{ handle = $p; verb = 'theme'; text = $Theme } | Out-Null
} else { Skip 'apply a theme' 'pass -Theme <file.thmx or .potx> to include it' }
Step 'export pptx' 'export' @{ handle = $p; format = 'pptx'; path = (Join-Path $out 'peak.pptx') } | Out-Null
Step 'export pdf' 'export' @{ handle = $p; format = 'pdf'; path = (Join-Path $out 'peak-deck.pdf') } | Out-Null

# ============================================================== done
$gateProc.StandardInput.Close()
if (-not $gateProc.WaitForExit(30000)) { $gateProc.Kill() }

$failed = @($script:results | Where-Object { -not $_.Ok })
$total = $script:results.Count
Write-Host ''
Write-Host ("{0} of {1} steps passed." -f ($total - $failed.Count), $total) -ForegroundColor $(if ($failed.Count) { 'Yellow' } else { 'Green' })
if ($failed.Count) {
    Write-Host 'Failures, in full:' -ForegroundColor Yellow
    foreach ($f in $failed) { Write-Host "  $($f.Label)`n    $($f.Text -replace "`n", "`n    ")" }
}
Write-Host "Exports are in $out. Excel, Word and PowerPoint were left open on the test documents: nothing was closed or saved over them."
exit $failed.Count
