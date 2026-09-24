<#
  new-testbed-docs.ps1 — build the throwaway documents a live run attaches to.

  Creates testbed/docs/{plan.xlsx,report.docx,deck.pptx} through real Office
  COM, the same automation surface the C# sidecar uses. Running this is
  therefore also the cheapest possible COM smoke test: if this fails, the
  sidecar was never going to work, and it fails in 20 lines instead of 300.

  Everything it writes is gitignored. Deleting testbed/docs and re-running is
  the intended way to reset a dirty run.

  TWO COM LESSONS, both learned by hanging this script on a real machine:

  1. Quit() on a dirty document raises a modal "Save changes?" prompt, and a
     modal dialog blocks every COM call into that app — including the Quit
     that caused it. The process then sits there unkillable-by-protocol, and
     the next run inherits the jam. Mark documents Saved before quitting and
     pass the do-not-save argument. This is precisely the hazard the C#
     sidecar's modal detection and soft timeouts exist to contain.

  2. Office COM servers are effectively single-instance per user. If Word is
     already open on YOUR documents, New-Object hands back THAT instance and
     Quit() closes your work. So this script refuses to run while Office is
     open rather than gambling with it. The sidecar attaches deliberately
     (GetActiveObject) and never quits what it did not start.

  Usage:  powershell -ExecutionPolicy Bypass -File scripts\new-testbed-docs.ps1
          -Visible   watch Office do it (default is headless)
          -Force     run even if Office is already open (DANGEROUS: may close
                     documents you have open; only for a known-clean machine)
#>
[CmdletBinding()]
param([switch]$Visible, [switch]$Force)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$docs = Join-Path $root 'testbed\docs'
New-Item -ItemType Directory -Force -Path $docs | Out-Null

# --- preflight: never hijack a running Office (see lesson 2) --------------
$running = Get-Process EXCEL, WINWORD, POWERPNT -ErrorAction SilentlyContinue
if ($running -and -not $Force) {
    Write-Host "Office is already running:" -ForegroundColor Yellow
    $running | ForEach-Object { Write-Host "  $($_.ProcessName) pid=$($_.Id) '$($_.MainWindowTitle)'" }
    throw "Close Office first, or pass -Force. COM would hand back your live instance and quitting it would close your documents."
}

# Office refuses to overwrite silently; clear first so a rerun is idempotent.
foreach ($n in 'plan.xlsx', 'report.docx', 'deck.pptx') {
    $p = Join-Path $docs $n
    if (Test-Path $p) { Remove-Item $p -Force }
}

function Release($o) {
    if ($null -ne $o) { try { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($o) } catch {} }
}

$made = @()

# --- Excel: the transfer SOURCE (a Q3 table the xfer op moves into Word) ---
$xl = $null; $wb = $null; $ws = $null
try {
    $xl = New-Object -ComObject Excel.Application
    $xl.Visible = [bool]$Visible
    $xl.DisplayAlerts = $false
    $wb = $xl.Workbooks.Add()
    $ws = $wb.Worksheets.Item(1)
    $ws.Name = 'Sheet1'
    $rows = @(
        @('Region', 'Q3 Revenue', 'Growth'),
        @('North',  '412000',     '0.12'),
        @('South',  '388500',     '0.07'),
        @('East',   '295750',     '-0.03'),
        @('West',   '501200',     '0.19')
    )
    for ($r = 0; $r -lt $rows.Count; $r++) {
        for ($c = 0; $c -lt $rows[$r].Count; $c++) {
            $ws.Cells.Item($r + 1, $c + 1).Value2 = $rows[$r][$c]
        }
    }
    $ws.Range('A1:C1').Font.Bold = $true
    $wb.SaveAs((Join-Path $docs 'plan.xlsx'), 51)   # 51 = xlOpenXMLWorkbook
    $wb.Close($false)
    $wb = $null
    $made += 'plan.xlsx   Sheet1!A1:C5 (5x3, header bold)'
}
finally {
    if ($xl) {
        try { $xl.DisplayAlerts = $false } catch {}
        try { foreach ($b in @($xl.Workbooks)) { $b.Saved = $true } } catch {}
        try { $xl.Quit() } catch {}
    }
    Release $ws; Release $wb; Release $xl
}

# --- Word: the transfer TARGET -------------------------------------------
$wd = $null; $doc = $null
try {
    $wd = New-Object -ComObject Word.Application
    $wd.Visible = [bool]$Visible
    $wd.DisplayAlerts = 0            # wdAlertsNone
    $doc = $wd.Documents.Add()
    $null = $doc.Content.InsertAfter('Q3 Regional Review')
    $doc.Paragraphs.Item(1).Range.Style = 'Heading 1'
    $null = $doc.Content.InsertParagraphAfter()
    $null = $doc.Content.InsertAfter('Placeholder body. The live run appends below this line.')
    # SaveAs2, not SaveAs: the [ref] overload cannot bind a PowerShell
    # PSObject-wrapped string and throws a type-conversion error.
    $doc.SaveAs2((Join-Path $docs 'report.docx'), 16)   # 16 = wdFormatDocumentDefault
    $doc.Saved = $true
    $doc.Close(0)                    # 0 = wdDoNotSaveChanges
    $doc = $null
    $made += 'report.docx 2 paragraphs (Heading 1 + body)'
}
finally {
    if ($wd) {
        # Lesson 1: clear the dirty bit BEFORE Quit or the save prompt hangs.
        try { foreach ($d in @($wd.Documents)) { $d.Saved = $true } } catch {}
        try { $wd.Quit(0) } catch {}
    }
    Release $doc; Release $wd
}

# --- PowerPoint: the deck target -----------------------------------------
$pp = $null; $pres = $null; $slide = $null
try {
    $pp = New-Object -ComObject PowerPoint.Application
    # PowerPoint has no headless mode: setting Visible=$false throws, so the
    # window always appears. Noted because it breaks the "-Visible controls
    # everything" expectation the other two apps honour.
    $pres = $pp.Presentations.Add($true)
    $slide = $pres.Slides.Add(1, 11)   # 11 = ppLayoutTitleOnly
    $slide.Shapes.Item(1).TextFrame.TextRange.Text = 'Q3 Regional Review'
    $pres.SaveAs((Join-Path $docs 'deck.pptx'), 24)   # 24 = ppSaveAsOpenXMLPresentation
    $pres.Close()
    $pres = $null
    $made += 'deck.pptx   1 slide (title only)'
}
finally {
    if ($pp) {
        try { foreach ($q in @($pp.Presentations)) { $q.Saved = $true } } catch {}
        try { $pp.Quit() } catch {}
    }
    Release $slide; Release $pres; Release $pp
}

[GC]::Collect(); [GC]::WaitForPendingFinalizers()

Write-Host ''
Write-Host "testbed docs written to $docs"
$made | ForEach-Object { Write-Host "  $_" }

# Orphan check: the supervisor contract the runbook's M1 acceptance demands.
Start-Sleep -Milliseconds 700
$orphans = Get-Process EXCEL, WINWORD, POWERPNT -ErrorAction SilentlyContinue
if ($orphans) {
    Write-Warning "orphan Office processes survived Quit(): $(($orphans | ForEach-Object { "$($_.ProcessName):$($_.Id)" }) -join ', ')"
    exit 1
}
Write-Host 'no orphan Office processes' -ForegroundColor Green
