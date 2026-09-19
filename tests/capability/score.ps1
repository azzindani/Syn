# score.ps1 - grade a capability run against tests/capability/SPEC.md.
#
# Everything here is read from the live documents and the saved transcript.
# Nothing is read from the model's own account of what it did: several runs
# have now reported work that had not happened.
#
#   powershell -File tests/capability/score.ps1
#   powershell -File tests/capability/score.ps1 -Chat .syn/chats/cXXX.jsonl

[CmdletBinding()]
param(
    [string]$Book = 'solar.xlsx',
    [string]$Memo = 'solar-memo.docx',
    [string]$Chat = ''
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$gt = Get-Content (Join-Path $PSScriptRoot 'ground-truth.json') -Raw | ConvertFrom-Json

$script:Score = 0
$script:Max = 0
$script:Rows = @()

function Check($id, $what, $points, [bool]$pass, $note) {
    $script:Max += $points
    if ($pass) { $script:Score += $points }
    $script:Rows += [pscustomobject]@{
        Check = $id; Pass = $(if ($pass) { 'PASS' } else { 'FAIL' })
        Got = "$(if ($pass) { $points } else { 0 })/$points"; What = $what; Note = $note
    }
}

# ---- the workbook --------------------------------------------------------
$xl = $null
try { $xl = [Runtime.InteropServices.Marshal]::GetActiveObject('Excel.Application') } catch {}
$wb = $null
if ($xl) { for ($i = 1; $i -le $xl.Workbooks.Count; $i++) { if ($xl.Workbooks.Item($i).Name -eq $Book) { $wb = $xl.Workbooks.Item($i) } } }
if (-not $wb) { throw "$Book is not open in Excel: the run cannot be scored" }

$sheets = @()
for ($i = 1; $i -le $wb.Worksheets.Count; $i++) { $sheets += $wb.Worksheets.Item($i).Name }

$summary = $wb.Worksheets | Where-Object { $_.Name -match 'summar' } | Select-Object -First 1
Check 'O1' 'a Summary sheet exists' 1 ($null -ne $summary) "sheets: $($sheets -join ', ')"

# O2/O3/O4 all read the same table, so find it once: the column holding the
# site names, and the first numeric column to its right.
$named = @{}
$formulaCells = 0
$valueCells = 0
if ($summary) {
    $used = $summary.UsedRange
    $rows = [Math]::Min($used.Rows.Count, 60)
    $cols = [Math]::Min($used.Columns.Count, 12)
    for ($r = 1; $r -le $rows; $r++) {
        $label = [string]$summary.Cells.Item($r, 1).Text
        if (-not $label) { continue }
        foreach ($site in $gt.per_site.PSObject.Properties.Name) {
            if ($label.Trim() -eq $site) {
                for ($c = 2; $c -le $cols; $c++) {
                    $cell = $summary.Cells.Item($r, $c)
                    $v = $cell.Value2
                    if ($v -is [double]) {
                        if (-not $named.ContainsKey($site)) {
                            $named[$site] = $v
                            if ($cell.HasFormula) { $formulaCells++ } else { $valueCells++ }
                        }
                        break
                    }
                }
            }
        }
    }
}
Check 'O2' 'the Summary names all 11 sites' 1 ($named.Count -eq $gt.sites) "found $($named.Count) of $($gt.sites)"
Check 'O3' 'its totals are live formulas, not pasted values' 2 ($formulaCells -gt 0 -and $formulaCells -ge $valueCells) "formula cells $formulaCells, pasted $valueCells"

$within = 0
$worst = ''
foreach ($site in $named.Keys) {
    $truth = $gt.per_site.$site.total
    $got = $named[$site]
    if ($truth -ne 0 -and ([Math]::Abs($got - $truth) / $truth) -le 0.01) { $within++ }
    elseif (-not $worst) { $worst = "$site got $([Math]::Round($got,1)) want $truth" }
}
Check 'O4' 'every per-site total is within 1% of ground truth' 2 ($named.Count -eq $gt.sites -and $within -eq $gt.sites) "$within of $($named.Count) within 1%. $worst"

# O5: a real PivotTable anywhere, or a grid with >= 12 month-looking rows.
$pivots = 0
$monthRows = 0
for ($i = 1; $i -le $wb.Worksheets.Count; $i++) {
    $ws = $wb.Worksheets.Item($i)
    try { $pivots += $ws.PivotTables().Count } catch {}
    $u = $ws.UsedRange
    $rr = [Math]::Min($u.Rows.Count, 200)
    $cc = [Math]::Min($u.Columns.Count, 6)
    $hits = 0
    for ($r = 1; $r -le $rr; $r++) {
        for ($c = 1; $c -le $cc; $c++) {
            $t = [string]$ws.Cells.Item($r, $c).Text
            if ($t -match '^\s*(20\d\d[-/]\d\d|\d\d?[-/]20\d\d)\s*$' -or
                $t -match '^(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)') { $hits++; break }
        }
    }
    if ($hits -gt $monthRows) { $monthRows = $hits }
}
Check 'O5' 'a monthly view exists (pivot or >= 12 month rows)' 1 ($pivots -gt 0 -or $monthRows -ge 12) "pivot tables $pivots, month-like rows $monthRows"

$dash = $wb.Worksheets | Where-Object { $_.Name -match 'dash' } | Select-Object -First 1
$charts = 0
if ($dash) { $charts = $dash.ChartObjects().Count }
Check 'O6' 'a Dashboard sheet carries >= 2 charts' 1 ($null -ne $dash -and $charts -ge 2) "dashboard: $($null -ne $dash), charts: $charts"

$bold = $false
$thousands = $false
if ($summary) {
    for ($c = 1; $c -le 8; $c++) { if ($summary.Cells.Item(1, $c).Font.Bold -eq $true) { $bold = $true } }
    for ($c = 1; $c -le 8; $c++) {
        $nf = [string]$summary.Cells.Item(2, $c).NumberFormat
        if ($nf -match '#,##') { $thousands = $true }
    }
}
Check 'O7' 'header bold and a kWh column with a thousands separator' 1 ($bold -and $thousands) "bold: $bold, thousands: $thousands"

# ---- the memo ------------------------------------------------------------
$wd = $null
try { $wd = [Runtime.InteropServices.Marshal]::GetActiveObject('Word.Application') } catch {}
$doc = $null
if ($wd) { for ($i = 1; $i -le $wd.Documents.Count; $i++) { if ($wd.Documents.Item($i).Name -eq $Memo) { $doc = $wd.Documents.Item($i) } } }
$paras = @()
if ($doc) { for ($i = 1; $i -le $doc.Paragraphs.Count; $i++) { $t = $doc.Paragraphs.Item($i).Range.Text.Trim(); if ($t) { $paras += $t } } }
$body = ($paras | Select-Object -Skip 1) -join ' '
Check 'O8' 'the memo has >= 5 non-empty paragraphs past its title' 1 ($paras.Count -ge 6) "non-empty paragraphs: $($paras.Count)"

Check 'J1' 'the memo names the correct top site' 2 ($body -like "*$($gt.top_site)*") "looking for '$($gt.top_site)'"

# Direction matters: summer high, winter low. Naming both seasons without the
# right order is not a finding.
$summer = $body -match '(?i)summer|june|july|august|jun\b|jul\b|aug\b'
$winter = $body -match '(?i)winter|december|january|november|dec\b|jan\b|nov\b'
Check 'J2' 'the memo states the seasonal swing with the right direction' 2 ($summer -and $winter) "summer mentioned: $summer, winter mentioned: $winter"

# ---- the process ---------------------------------------------------------
if (-not $Chat) {
    $Chat = (Get-ChildItem (Join-Path $repo '.syn\chats') -Filter *.jsonl | Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
}
$issued = 0; $executed = 0; $declined = 0; $shell = 0; $doom = $false; $answered = $false; $stopNote = ''
foreach ($line in Get-Content $Chat) {
    try { $m = $line | ConvertFrom-Json } catch { continue }
    switch ($m.role) {
        'calls' {
            try { $arr = $m.text | ConvertFrom-Json; $issued += $arr.Count; foreach ($c in $arr) { if ($c.function.name -eq 'shell') { $shell++ } } } catch {}
        }
        'tool' {
            if ($m.text -like '*not run:*') { $declined++ } else { $executed++ }
            if ($m.text -like '*same op+args*') { $doom = $true }
        }
        'assistant' {
            if ($m.text -like '`[run stopped:*') { $stopNote = $m.text }
            elseif ($m.text.Trim()) { $answered = $true }
        }
    }
}
# The stop reason is written into the transcript, so a run that ended on the
# budget says so in the chat. The first version of this grepped for a console
# line the chat never holds, and so passed a run whose budget was spent.
$budget = -not ($stopNote -like '*step budget spent*')

Check 'P1' 'every call issued was executed (no silent drops)' 1 ($issued -gt 0 -and ($executed + $declined) -eq $issued) "issued $issued, answered $($executed + $declined)"
$rate = if ($issued -gt 0) { [Math]::Round(100.0 * $executed / $issued, 1) } else { 0 }
Check 'P2' '>= 90% of calls executed rather than declined' 1 ($rate -ge 90) "$rate% executed ($executed of $issued)"
Check 'P3' 'the doom-loop gate never fired' 1 (-not $doom) $(if ($doom) { 'repeated an identical call 3x' } else { 'no repeats' })
Check 'P4' 'no shell attempt: it stayed inside the document ops' 1 ($shell -eq 0) "shell calls: $shell"
Check 'P5' 'ended with a prose answer' 1 $answered $(if ($answered) { 'answered' } else { 'stopped or blank' })
Check 'P6' 'finished inside the step budget' 1 $budget $(if ($budget) { 'within budget' } else { 'budget spent' })

# ---- verdict -------------------------------------------------------------
$script:Rows | Format-Table -AutoSize -Wrap
$verdict = if ($script:Score -ge 17) { 'the concept holds' }
    elseif ($script:Score -ge 12) { 'the harness works, the output needs a human pass' }
    elseif ($script:Score -ge 6) { 'parts drive, the job does not complete' }
    else { 'the concept does not survive a real task' }
Write-Host ""
Write-Host "SCORE $($script:Score)/$($script:Max)  --  $verdict"
Write-Host "transcript: $Chat"
