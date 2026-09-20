# setup.ps1 - put the fixture in front of the harness, and nothing else.
#
# This script must never write any part of the expected output: no Summary
# sheet, no Dashboard, no memo body. It loads the data, opens the memo, and
# starts the two sidecars. Anything beyond that would be marking its own
# homework (see SPEC.md, "Honesty conditions").

[CmdletBinding()]
param([switch]$SkipReload)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$csv = Join-Path $repo 'samples\Solar_Energy_Production.csv'
$book = Join-Path $repo 'testbed\docs\solar.xlsx'
$memo = Join-Path $repo 'testbed\docs\solar-memo.docx'
$deck = Join-Path $repo 'testbed\docs\solar-deck.pptx'
$exe = Join-Path $repo 'sidecar-csharp\Host\bin\Release\net8.0-windows\office-host.exe'
if (-not (Test-Path $csv)) { throw "fixture missing: $csv" }
if (-not (Test-Path $exe)) { throw "sidecar not built: $exe" }

Get-Process office-host -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 600

$M = [System.Reflection.Missing]::Value
$xl = $null
try { $xl = [Runtime.InteropServices.Marshal]::GetActiveObject('Excel.Application') } catch {}
if (-not $xl) { $xl = New-Object -ComObject Excel.Application }
$xl.Visible = $true
$xl.DisplayAlerts = $false
for ($i = $xl.Workbooks.Count; $i -ge 1; $i--) {
    if ($xl.Workbooks.Item($i).Name -eq 'solar.xlsx') { $xl.Workbooks.Item($i).Close($false) }
}

if (-not $SkipReload -or -not (Test-Path $book)) {
    Write-Host "loading $csv ..."
    $wb = $xl.Workbooks.Open($csv)
    $ws = $wb.Worksheets.Item(1)
    $ws.Name = 'data'
    # This locale reads "1.130" as one thousand one hundred and thirty unless
    # the separators are stated, which silently corrupts every kWh reading.
    $ws.Columns.Item(1).TextToColumns($ws.Range('A1'), 1, 1, $false, $false, $false, $true, $false, $false, $M, $M, '.', ',') | Out-Null
    if (Test-Path $book) { Remove-Item $book -Force }
    $wb.SaveAs($book, 51)
} else {
    $wb = $xl.Workbooks.Open($book)
}

# Back to one sheet: a previous run's Summary and Dashboard would hand the
# next run a pass it did not earn.
for ($i = $wb.Worksheets.Count; $i -ge 1; $i--) {
    if ($wb.Worksheets.Item($i).Name -ne 'data') { $wb.Worksheets.Item($i).Delete() }
}
$wb.Save()
$rows = $wb.Worksheets.Item('data').UsedRange.Rows.Count
Write-Host "workbook: $($wb.Name), sheets: $($wb.Worksheets.Count), rows: $rows"

# A blank memo with room to write in. `insertParagraph` is not wired for
# Word, so the paragraphs have to exist for the model to fill them.
#
# Copied from a committed template rather than built here: Word's SaveAs2
# hangs behind an invisible dialog when it is called from a child process,
# and a setup step that blocks forever is worse than a file in the repo.
Write-Host 'word: opening the memo'
$tpl = Join-Path $PSScriptRoot 'memo-template.docx'
if (-not (Test-Path $tpl)) { throw "memo template missing: $tpl" }
$wd = $null
try { $wd = [Runtime.InteropServices.Marshal]::GetActiveObject('Word.Application') } catch {}
if (-not $wd) { $wd = New-Object -ComObject Word.Application }
$wd.Visible = $true
$wd.DisplayAlerts = 0
for ($i = $wd.Documents.Count; $i -ge 1; $i--) {
    if ($wd.Documents.Item($i).Name -eq 'solar-memo.docx') { $wd.Documents.Item($i).Close($false) }
}
Copy-Item $tpl $memo -Force
$doc = $wd.Documents.Open($memo)
Write-Host "memo: $($doc.Name), paragraphs: $($doc.Paragraphs.Count)"

# An empty 16:9 deck. Unlike the memo this needs no blank placeholders:
# createSlide appends, so the deck starts with nothing in it and the run
# has to build every slide itself.
Write-Host 'powerpoint: opening the deck'
$dtpl = Join-Path $PSScriptRoot 'deck-template.pptx'
if (-not (Test-Path $dtpl)) { throw "deck template missing: $dtpl" }
$pp = $null
try { $pp = [Runtime.InteropServices.Marshal]::GetActiveObject('PowerPoint.Application') } catch {}
if (-not $pp) { $pp = New-Object -ComObject PowerPoint.Application }
$pp.Visible = -1        # MsoTriState, not a boolean
for ($i = $pp.Presentations.Count; $i -ge 1; $i--) {
    if ($pp.Presentations.Item($i).Name -eq 'solar-deck.pptx') { $pp.Presentations.Item($i).Close() }
}
Copy-Item $dtpl $deck -Force
$pres = $pp.Presentations.Open($deck)
Write-Host "deck: $($pres.Name), slides: $($pres.Slides.Count)"

Start-Process -FilePath $exe -ArgumentList '--pipe', 'hand-excel', '--app', 'excel' -WindowStyle Hidden
Start-Process -FilePath $exe -ArgumentList '--pipe', 'hand-word', '--app', 'word' -WindowStyle Hidden
Start-Process -FilePath $exe -ArgumentList '--pipe', 'hand-ppt', '--app', 'powerpoint' -WindowStyle Hidden
Start-Sleep -Seconds 5
Write-Host "sidecars up. Ready for the brief."
