# score-v2.ps1 - grade a run against tests/capability/SPEC-v2.md (80 points).
#
# Every artifact check reads the live documents. Every process check reads
# the saved transcript. Nothing is graded from the model's own account of
# what it did: several runs have now reported work that had not happened.
# The four judgement checks do read the report's prose, because judging the
# writing is the point of them -- they are the only ones that do.
#
#   powershell -File tests/capability/score-v2.ps1
#   powershell -File tests/capability/score-v2.ps1 -Chat .syn\chats\cXXX.jsonl
#   powershell -File tests/capability/score-v2.ps1 -Control

[CmdletBinding()]
param(
    [string]$Book = 'solar.xlsx',
    [string]$Report = 'solar-memo.docx',
    [string]$Chat = '',
    # A control run is scripted, not driven by the model, so it has no
    # transcript. Scoring it against the last model run's process numbers
    # would report a total nobody earned.
    [switch]$Control
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$gt = Get-Content (Join-Path $PSScriptRoot 'ground-truth.json') -Raw | ConvertFrom-Json

$script:Score = 0
$script:Max = 0
# Named Card, not Rows: PowerShell variable names are case-insensitive, so a
# local $rows counting spreadsheet rows silently overwrote $script:Rows once
# and the rubric blew up on an integer halfway down.
$script:Card = New-Object System.Collections.Generic.List[object]

function Check([string]$id, [string]$what, [int]$points, [bool]$pass, [string]$note) {
    $script:Max += $points
    if ($pass) { $script:Score += $points }
    $script:Card.Add([pscustomobject]@{
        Check = $id
        Pass  = $(if ($pass) { 'PASS' } else { 'FAIL' })
        Got   = "$(if ($pass) { $points } else { 0 })/$points"
        What  = $what
        Note  = $note
    }) | Out-Null
}

# Text that marks the number after it as a ratio rather than a quantity.
$FactorOf = '(?i)(factor|ratio|multiple)\s+of\s+$'

function Near($got, $want, [double]$tol = 0.01) {
    if ($null -eq $got -or $got -isnot [double]) { return $false }
    if ($want -eq 0) { return [Math]::Abs($got) -lt 1e-6 }
    return ([Math]::Abs($got - $want) / [Math]::Abs($want)) -le $tol
}

# ---- the workbook --------------------------------------------------------
$xl = $null
try { $xl = [Runtime.InteropServices.Marshal]::GetActiveObject('Excel.Application') } catch {}
$wb = $null
if ($xl) { for ($i = 1; $i -le $xl.Workbooks.Count; $i++) { if ($xl.Workbooks.Item($i).Name -eq $Book) { $wb = $xl.Workbooks.Item($i) } } }
if (-not $wb) { throw "$Book is not open in Excel: the run cannot be scored" }

$sheets = @()
for ($i = 1; $i -le $wb.Worksheets.Count; $i++) { $sheets += [string]$wb.Worksheets.Item($i).Name }

# X1: the raw data as a real Table, not a bare range.
$listObjects = 0
$bigTable = $false
for ($i = 1; $i -le $wb.Worksheets.Count; $i++) {
    $lo = $wb.Worksheets.Item($i).ListObjects
    $listObjects += [int]$lo.Count
    for ($j = 1; $j -le $lo.Count; $j++) {
        if ($lo.Item($j).Range.Rows.Count -ge 100000) { $bigTable = $true }
    }
}
Check 'X1' 'the raw data is a real Excel Table, not a bare range' 2 $bigTable "ListObjects: $listObjects, one over 100k rows: $bigTable"

# X2: derived year/month/hour/season columns, and they are formulas.
$derived = @{}
for ($i = 1; $i -le $wb.Worksheets.Count; $i++) {
    $ws = $wb.Worksheets.Item($i)
    if ($ws.UsedRange.Rows.Count -lt 100000) { continue }
    $cols = [Math]::Min($ws.UsedRange.Columns.Count, 30)
    for ($c = 1; $c -le $cols; $c++) {
        $head = ([string]$ws.Cells.Item(1, $c).Text).Trim().ToLower()
        if ($head -match '^(year|month|hour|season)$' -and -not $derived.ContainsKey($head)) {
            $derived[$head] = [bool]$ws.Cells.Item(2, $c).HasFormula
        }
    }
}
$derivedOk = @($derived.Keys | Where-Object { $derived[$_] }).Count
$derivedNames = ($derived.Keys | Sort-Object) -join ','
Check 'X2' 'derived year, month, hour and season columns, as formulas' 3 ($derivedOk -ge 4) "found $($derived.Count) [$derivedNames], of which $derivedOk are formulas"

# X3/X4/X5/X6 all read one scorecard, so find it once: a sheet holding site
# names down a column, with numbers to their right. Called $board, not
# $card: PowerShell names are case-insensitive and $card silently became
# $script:Card, the rubric itself.
$board = $null
$boardCol = 1
$bestScore = -1
foreach ($nm in $sheets) {
    $ws = $wb.Worksheets.Item($nm)
    if ($ws.UsedRange.Rows.Count -gt 5000) { continue }
    $rr = [Math]::Min($ws.UsedRange.Rows.Count, 80)
    $cc = [Math]::Min($ws.UsedRange.Columns.Count, 10)
    for ($c = 1; $c -le $cc; $c++) {
        $hits = 0
        for ($r = 1; $r -le $rr; $r++) {
            $t = ([string]$ws.Cells.Item($r, $c).Text).Trim()
            if ($t -and $gt.per_site.PSObject.Properties.Name -contains $t) { $hits++ }
        }
        # The best candidate, not the first. A run that abandons one attempt
        # and builds the real scorecard on the next sheet used to be graded
        # on the wreckage: "Scorecard" held eleven site names and one stray
        # column, "Scorecard2" held the seven live formulas, and the first
        # match won.
        if ($hits -ge 8) {
            $numeric = 0
            for ($rr2 = 1; $rr2 -le $rr; $rr2++) {
                for ($cc2 = $c + 1; $cc2 -le $cc; $cc2++) {
                    if ($ws.Cells.Item($rr2, $cc2).Value2 -is [double]) { $numeric++ }
                }
            }
            if ($numeric -gt $bestScore) { $bestScore = $numeric; $board = $ws; $boardCol = $c }
        }
    }
}

$metrics = @{}   # site -> array of the numeric cells to the right
$formulaCells = 0
$valueCells = 0
$metricCols = 0
if ($board) {
    $rr = [Math]::Min($board.UsedRange.Rows.Count, 80)
    $cc = [Math]::Min($board.UsedRange.Columns.Count, 16)
    for ($r = 1; $r -le $rr; $r++) {
        $label = ([string]$board.Cells.Item($r, $boardCol).Text).Trim()
        if (-not $label -or $gt.per_site.PSObject.Properties.Name -notcontains $label) { continue }
        $vals = @()
        for ($c = $boardCol + 1; $c -le $cc; $c++) {
            $cell = $board.Cells.Item($r, $c)
            $v = $cell.Value2
            if ($v -is [double]) {
                $vals += $v
                if ($cell.HasFormula) { $formulaCells++ } else { $valueCells++ }
            }
        }
        if ($vals.Count -gt $metricCols) { $metricCols = $vals.Count }
        if (-not $metrics.ContainsKey($label)) { $metrics[$label] = $vals }
    }
}
Check 'X3' 'a per-site scorecard names all 11 sites' 2 ($metrics.Count -eq $gt.sites) "sheet: $(if($board){$board.Name}else{'none found'}), sites: $($metrics.Count) of $($gt.sites)"
Check 'X4' 'it carries >= 6 metric columns, all live formulas' 4 ($metricCols -ge 6 -and $valueCells -eq 0 -and $formulaCells -gt 0) "widest row: $metricCols metrics, formula cells $formulaCells, pasted $valueCells"

# X5: every one of total, mean, median, p95, peak and count within 1%, for
# every site. Column order is the model's choice, so each truth value is
# looked for anywhere in that site's row.
$missing = @()
foreach ($site in $gt.per_site_detail.PSObject.Properties.Name) {
    $d = $gt.per_site_detail.$site
    $got = if ($metrics.ContainsKey($site)) { $metrics[$site] } else { @() }
    foreach ($m in 'total', 'mean', 'median', 'p95', 'peak', 'rows') {
        $want = $d.$m
        $found = $false
        foreach ($g in $got) { if (Near $g $want) { $found = $true; break } }
        if (-not $found) { $missing += "$site/$m" }
    }
}
Check 'X5' 'total, mean, median, p95, peak and count within 1% for every site' 5 ($missing.Count -eq 0) "$(66 - $missing.Count) of 66 metrics matched$(if($missing.Count){'. first miss: ' + $missing[0]})"

# X6: the share column sums to 1 (or to 100).
$shareOk = $false
if ($board -and $metricCols -gt 0) {
    for ($c = $boardCol + 1; $c -le $boardCol + 16; $c++) {
        $sum = 0.0; $n = 0
        for ($r = 1; $r -le [Math]::Min($board.UsedRange.Rows.Count, 80); $r++) {
            $label = ([string]$board.Cells.Item($r, $boardCol).Text).Trim()
            if ($gt.per_site.PSObject.Properties.Name -notcontains $label) { continue }
            $v = $board.Cells.Item($r, $c).Value2
            if ($v -is [double]) { $sum += $v; $n++ }
        }
        if ($n -ge 11 -and ([Math]::Abs($sum - 1) -le 0.005 -or [Math]::Abs($sum - 100) -le 0.5)) { $shareOk = $true }
    }
}
Check 'X6' 'a share-of-estate column that sums to 100%' 1 $shareOk "looking for a column over the 11 sites summing to 1 or 100"

# X7: >= 91 year-month rows somewhere, or a pivot standing in for them.
$pivots = 0
$monthRows = 0
for ($i = 1; $i -le $wb.Worksheets.Count; $i++) {
    $ws = $wb.Worksheets.Item($i)
    try { $pivots += [int]$ws.PivotTables().Count } catch {}
    if ($ws.UsedRange.Rows.Count -gt 5000) { continue }
    $rr = [Math]::Min($ws.UsedRange.Rows.Count, 400)
    $cc = [Math]::Min($ws.UsedRange.Columns.Count, 8)
    $hits = 0
    for ($r = 2; $r -le $rr; $r++) {
        for ($c = 1; $c -le $cc; $c++) {
            $t = ([string]$ws.Cells.Item($r, $c).Text).Trim()
            if ($t -match '^(19|20)\d\d$' -or $t -match '^\s*20\d\d[-/]\d\d?\s*$') { $hits++; break }
        }
    }
    if ($hits -gt $monthRows) { $monthRows = $hits }
}
Check 'X7' 'a monthly breakdown of >= 91 year-months, or a pivot' 2 ($monthRows -ge 91 -or $pivots -ge 1) "year-month-like rows: $monthRows, pivots: $pivots"

# X8: four seasons, each within 1%.
$seasonHit = 0
# Excel's own date grouping says "Fall" where the fixture says "Autumn".
# The check is whether the four seasonal totals are right, not whether the
# run picked the same dialect as the ground truth file.
$seasonAlias = @{ 'Autumn' = @('Autumn', 'Fall'); 'Winter' = @('Winter'); 'Spring' = @('Spring'); 'Summer' = @('Summer') }
foreach ($s in $gt.by_season.PSObject.Properties.Name) {
    $want = $gt.by_season.$s
    $names = if ($seasonAlias.ContainsKey($s)) { $seasonAlias[$s] } else { @($s) }
    $found = $false
    foreach ($nm in $sheets) {
        $ws = $wb.Worksheets.Item($nm)
        if ($ws.UsedRange.Rows.Count -gt 5000) { continue }
        $rr = [Math]::Min($ws.UsedRange.Rows.Count, 200)
        $cc = [Math]::Min($ws.UsedRange.Columns.Count, 12)
        for ($r = 1; $r -le $rr -and -not $found; $r++) {
            for ($c = 1; $c -le $cc; $c++) {
                if ($names -contains ([string]$ws.Cells.Item($r, $c).Text).Trim()) {
                    for ($k = $c + 1; $k -le $cc; $k++) {
                        if (Near $ws.Cells.Item($r, $k).Value2 $want) { $found = $true; break }
                    }
                }
                if ($found) { break }
            }
        }
        if ($found) { break }
    }
    if ($found) { $seasonHit++ }
}
Check 'X8' 'a seasonal breakdown, four seasons, each within 1%' 2 ($seasonHit -eq 4) "$seasonHit of 4 season totals matched"

# X9: an hour-of-day profile whose largest row is the true peak hour.
$hourRows = 0
$hourPeak = -1
foreach ($nm in $sheets) {
    $ws = $wb.Worksheets.Item($nm)
    if ($ws.UsedRange.Rows.Count -gt 5000) { continue }
    $rr = [Math]::Min($ws.UsedRange.Rows.Count, 200)
    $cc = [Math]::Min($ws.UsedRange.Columns.Count, 12)
    for ($c = 1; $c -le $cc; $c++) {
        $seen = @{}; $best = -1; $bestHour = -1
        for ($r = 2; $r -le $rr; $r++) {
            $h = $ws.Cells.Item($r, $c).Value2
            if ($h -isnot [double] -or $h -lt 0 -or $h -gt 23 -or $h -ne [Math]::Floor($h)) { continue }
            if ($seen.ContainsKey([int]$h)) { continue }
            $seen[[int]$h] = $true
            for ($k = $c + 1; $k -le $cc; $k++) {
                $v = $ws.Cells.Item($r, $k).Value2
                if ($v -is [double] -and $v -gt $best) { $best = $v; $bestHour = [int]$h }
                if ($v -is [double]) { break }
            }
        }
        if ($seen.Count -gt $hourRows) { $hourRows = $seen.Count; $hourPeak = $bestHour }
    }
}
Check 'X9' 'an hour-of-day profile, >= 12 rows, peak hour correct' 2 ($hourRows -ge 12 -and $hourPeak -eq [int]$gt.peak_hour) "$hourRows hour rows, peak hour $hourPeak, truth $($gt.peak_hour)"

Check 'X10' '>= 2 PivotTables in the workbook' 2 ($pivots -ge 2) "pivot tables: $pivots"

$slicers = 0
try { $slicers = [int]$wb.SlicerCaches.Count } catch {}
Check 'X11' '>= 1 slicer, wired to a pivot' 2 ($slicers -ge 1) "slicer caches: $slicers"

# X12: at least two conditional-format rules, of at least two kinds.
$cfKinds = @{}
$cfRules = 0
foreach ($nm in $sheets) {
    $ws = $wb.Worksheets.Item($nm)
    if ($ws.UsedRange.Rows.Count -gt 5000) { continue }
    try {
        $fc = $ws.Cells.FormatConditions
        for ($i = 1; $i -le $fc.Count; $i++) {
            $cfRules++
            $cfKinds[[string]$fc.Item($i).Type] = $true
        }
    } catch {}
}
Check 'X12' 'conditional formatting: >= 2 rules of >= 2 kinds' 2 ($cfRules -ge 2 -and $cfKinds.Count -ge 2) "$cfRules rules, $($cfKinds.Count) distinct kinds"

Check 'X13' '>= 1 named range' 1 ($wb.Names.Count -ge 1) "named ranges: $($wb.Names.Count)"

# ---- the dashboard -------------------------------------------------------
$dash = $null
$dashCharts = 0
foreach ($nm in $sheets) {
    $ws = $wb.Worksheets.Item($nm)
    $n = 0
    try { $n = [int]$ws.ChartObjects().Count } catch {}
    if ($n -gt $dashCharts -and $ws.UsedRange.Rows.Count -lt 100000) { $dashCharts = $n; $dash = $ws }
}
Check 'D1' 'a dashboard sheet exists and is not the data sheet' 1 ($null -ne $dash) "sheet: $(if($dash){$dash.Name}else{'none'})"
Check 'D2' 'it carries >= 5 chart objects' 3 ($dashCharts -ge 5) "charts: $dashCharts"

$box = @()
$types = @{}
$titled = 0
if ($dash) {
    $co = $dash.ChartObjects()
    for ($i = 1; $i -le $co.Count; $i++) {
        $c = $co.Item($i)
        $types[[string]$c.Chart.ChartType] = $true
        try { if ($c.Chart.HasTitle -and ([string]$c.Chart.ChartTitle.Text).Trim()) { $titled++ } } catch {}
        $box += , @([double]$c.Left, [double]$c.Top, [double]$c.Width, [double]$c.Height)
    }
}
Check 'D3' '>= 3 distinct chart types' 2 ($types.Count -ge 3) "distinct types: $($types.Count)"
Check 'D4' 'every chart carries a title of its own' 2 ($dashCharts -gt 0 -and $titled -eq $dashCharts) "$titled of $dashCharts titled"

$overlaps = 0
for ($i = 0; $i -lt $box.Count; $i++) {
    for ($j = $i + 1; $j -lt $box.Count; $j++) {
        $a = $box[$i]; $b = $box[$j]
        if (($a[0] -lt $b[0] + $b[2]) -and ($b[0] -lt $a[0] + $a[2]) -and
            ($a[1] -lt $b[1] + $b[3]) -and ($b[1] -lt $a[1] + $a[3])) { $overlaps++ }
    }
}
Check 'D5' 'the charts are laid out: no two overlap' 2 ($box.Count -gt 0 -and $overlaps -eq 0) "overlapping pairs: $overlaps"

# ---- the report ----------------------------------------------------------
$wd = $null
try { $wd = [Runtime.InteropServices.Marshal]::GetActiveObject('Word.Application') } catch {}
$doc = $null
if ($wd) { for ($i = 1; $i -le $wd.Documents.Count; $i++) { if ($wd.Documents.Item($i).Name -eq $Report) { $doc = $wd.Documents.Item($i) } } }

$pages = 0; $words = 0; $headings = 0; $tables = 0; $shapes = 0; $tocs = 0; $pageField = $false
$body = ''
if ($doc) {
    try { $doc.Repaginate() } catch {}
    $pages = [int]$doc.ComputeStatistics(2)   # wdStatisticPages
    $words = [int]$doc.ComputeStatistics(0)   # wdStatisticWords
    $tables = [int]$doc.Tables.Count
    $shapes = [int]$doc.InlineShapes.Count
    $tocs = [int]$doc.TablesOfContents.Count
    $sb = New-Object System.Text.StringBuilder
    for ($i = 1; $i -le $doc.Paragraphs.Count; $i++) {
        $p = $doc.Paragraphs.Item($i)
        $t = $p.Range.Text.Trim()
        if (-not $t) { continue }
        if (([string]$p.Style.NameLocal) -match '^Heading \d') { $headings++ }
        [void]$sb.Append($t).Append(' ')
    }
    $body = $sb.ToString()
    for ($s = 1; $s -le $doc.Sections.Count -and -not $pageField; $s++) {
        foreach ($part in @($doc.Sections.Item($s).Footers, $doc.Sections.Item($s).Headers)) {
            for ($h = 1; $h -le 3; $h++) {
                $f = $part.Item($h).Range.Fields
                for ($k = 1; $k -le $f.Count; $k++) { if ([int]$f.Item($k).Type -eq 33) { $pageField = $true } }
            }
        }
    }
}
Check 'W1' '>= 20 pages' 4 ($pages -ge 20) "pages: $pages"
Check 'W2' '>= 8 paragraphs styled as a real Heading level' 3 ($headings -ge 8) "heading-styled paragraphs: $headings"
Check 'W3' 'a table of contents field' 2 ($tocs -ge 1) "TOC fields: $tocs"

$bigTables = 0
if ($doc) { for ($i = 1; $i -le $doc.Tables.Count; $i++) { if ($doc.Tables.Item($i).Rows.Count -ge 3) { $bigTables++ } } }
Check 'W4' '>= 3 Word tables of >= 3 rows each' 3 ($bigTables -ge 3) "$bigTables tables of 3+ rows (of $tables)"
Check 'W5' '>= 4 pictures or inline shapes' 3 ($shapes -ge 4) "inline shapes: $shapes"
Check 'W6' 'a header or footer carrying a page-number field' 2 $pageField "PAGE field found: $pageField"
Check 'W7' '>= 2,500 words of body text' 3 ($words -ge 2500) "words: $words"

# ---- judgement -----------------------------------------------------------
# These four read the prose on purpose: whether the writing says anything
# true is what they are for. Everything above reads the documents.
$share = [Math]::Round(100.0 * $gt.top_site_kwh / $gt.grand_total_kwh, 0)
$namesTop = $body -like "*$($gt.top_site)*"
$saysShare = $body -match "\b(3[01](\.\d)?)\s*(%|per cent|percent)"
Check 'J1' 'names the top site and its share of estate output' 2 ($namesTop -and $saysShare) "names '$($gt.top_site)': $namesTop, quotes ~$share%: $saysShare"

$summer = $body -match '(?i)\b(summer|july|august|jun)'
$winter = $body -match '(?i)\b(winter|december|january|nov)'
# "a factor of 6.7" is how anyone actually writes this, and the first
# version of this pattern recognised only "6.7x" and "6.7-fold".
$magnitude = $body -match '(?i)(\b\d{1,2}(\.\d)?\s*(x\b|-fold|fold|times)|factor of\s+\d)'
Check 'J2' 'states the seasonal swing, right direction and magnitude' 2 ($summer -and $winter -and $magnitude) "summer $summer, winter $winter, a multiple quoted $magnitude"

$peak = $body -match "(?i)\b(13:00|1300|13h|1 ?pm|hour 13)\b"
Check 'J3' 'identifies the peak generating hour correctly' 2 $peak "looking for 13:00 in the prose"

$partial = ($body -match '(?i)\b2015\b') -and ($body -match '(?i)\b2023\b') -and
           ($body -match '(?i)(partial|incomplete|part[- ]year|not a full year|truncat)')
Check 'J4' 'flags 2015 and 2023 as partial years' 2 $partial "both years named and called partial: $partial"

# J5: every large kWh figure in the prose should be a number the workbook
# actually holds. Read from the workbook, never from the model's summary.
$truths = New-Object System.Collections.Generic.List[double]
$truths.Add([double]$gt.grand_total_kwh)
foreach ($s in $gt.per_site_detail.PSObject.Properties.Name) {
    $truths.Add([double]$gt.per_site_detail.$s.total)
    $truths.Add([double]$gt.per_site_detail.$s.peak)
}
foreach ($s in $gt.by_season.PSObject.Properties.Name) { $truths.Add([double]$gt.by_season.$s) }
foreach ($s in $gt.by_year.PSObject.Properties.Name) { $truths.Add([double]$gt.by_year.$s) }
foreach ($s in $gt.by_calendar_month.PSObject.Properties.Name) { $truths.Add([double]$gt.by_calendar_month.$s) }
foreach ($s in $gt.by_hour.PSObject.Properties.Name) { $truths.Add([double]$gt.by_hour.$s) }
# Reading counts are figures the report legitimately quotes, and leaving
# them out of the truth set scored a correct number as a miss. The check
# is "figures in the prose match the data"; a row count is data.
$allRows = 0
foreach ($s in $gt.per_site_detail.PSObject.Properties.Name) {
    $truths.Add([double]$gt.per_site_detail.$s.rows)
    $allRows += [double]$gt.per_site_detail.$s.rows
}
$truths.Add($allRows)

$quoted = 0; $matched = 0; $firstBad = ''
foreach ($m in [regex]::Matches($body, '\b\d{1,3}(?:,\d{3})+(?:\.\d+)?\b')) {
    $v = [double]($m.Value -replace ',', '')
    if ($v -lt 1000) { continue }
    # "a factor of 2,310" is a ratio, not a kWh figure, and the check is for
    # kWh figures. Without this a correct report that quotes a large
    # multiple -- which the seasonal finding invites -- fails on it.
    if ($body.Substring(0, $m.Index) -match $FactorOf) { continue }
    $quoted++
    $ok = $false
    foreach ($t in $truths) { if (Near $v $t) { $ok = $true; break } }
    if ($ok) { $matched++ } elseif (-not $firstBad) { $firstBad = $m.Value }
}
Check 'J5' 'every kWh figure in the prose matches the data to within 1%' 2 ($quoted -ge 5 -and $matched -eq $quoted) "$matched of $quoted thousands-separated figures matched$(if($firstBad){". first miss: $firstBad"})"

# ---- the process ---------------------------------------------------------
if ($Control) {
    Write-Host ''
    $script:Card | Format-Table -AutoSize -Wrap
    Write-Host "CONTROL $($script:Score)/$($script:Max) on artifacts and judgement."
    Write-Host 'This measures what the harness can express, not what a model can drive.'
    Write-Host 'It is not a capability-test result and does not count as a pass.'
    return
}
if (-not $Chat) {
    $Chat = (Get-ChildItem (Join-Path $repo '.syn\chats') -Filter *.jsonl | Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
}
$issued = 0; $executed = 0; $declined = 0; $shell = 0; $doom = $false; $answered = $false; $stopNote = ''
foreach ($line in Get-Content $Chat) {
    try { $m = $line | ConvertFrom-Json } catch { continue }
    switch ($m.role) {
        'calls' {
            try {
                $arr = $m.text | ConvertFrom-Json
                $issued += [int]@($arr).Count
                foreach ($c in $arr) { if ($c.function.name -eq 'shell') { $shell++ } }
            } catch {}
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

Check 'P1' 'every tool call issued was executed' 2 ($issued -gt 0 -and ($executed + $declined) -eq $issued) "issued $issued, answered $($executed + $declined)"
$rate = if ($issued -gt 0) { [Math]::Round(100.0 * $executed / $issued, 1) } else { 0 }
Check 'P2' '>= 90% executed rather than refused' 2 ($rate -ge 90) "$rate% executed ($executed of $issued)"
Check 'P3' 'the doom-loop gate never fired' 1 (-not $doom) $(if ($doom) { 'repeated an identical call 3x' } else { 'no repeats' })
Check 'P4' 'no shell attempt' 1 ($shell -eq 0) "shell calls: $shell"
Check 'P5' 'ended with a prose answer, not a stop' 2 $answered $(if ($answered) { 'answered' } else { $stopNote })
Check 'P6' 'finished inside the step budget' 2 $budget $(if ($budget) { 'within budget' } else { 'budget spent' })

# ---- verdict -------------------------------------------------------------
$script:Card | Format-Table -AutoSize -Wrap
$verdict = if ($script:Score -ge 68) { 'v0.1.0: a stakeholder could use this without knowing a model made it' }
    elseif ($script:Score -ge 52) { 'the engine is sound, the output needs an editor' }
    elseif ($script:Score -ge 30) { 'it builds parts of a review, not a review' }
    else { 'not yet an engine' }
Write-Host ''
Write-Host "SCORE $($script:Score)/$($script:Max)  --  $verdict"
Write-Host "transcript: $Chat"
