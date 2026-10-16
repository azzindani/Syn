<#
  live-layout.ps1 -- Syn beside Excel, on real windows.

  The status menu's "Windows" section asks the console to put Syn on one
  side of the screen and the Office application on the rest, edge to edge.
  The arithmetic is tested by cargo test; this proves the windows really
  land there: every window has an invisible resize border, a maximized or
  minimized one has to be brought back first, and a screen with scaling is
  measured in pixels the console must not be lied to about.

  It reads the rectangles back the way a person sees them (the drawn frame,
  without the invisible border) and checks, for several ratios on both sides:
    * Syn is the share of the screen's working area it was asked to be
      (or its own smallest width, and then Excel gets what is left);
    * Excel has the rest, touching Syn, with no gap and no overlap;
    * both fill the working area top to bottom;
    * a maximized Excel and a minimized one are both brought to the place;
    * a request outside what makes a layout, or one with no Origin or the
      wrong one, is refused and moves nothing.
  The windows are put back where they were, and an Excel this script
  started is ended (its workbook discarded). One that was running stays.
  The exit code is the number of failures.

  Needs Syn running in a window of its own (the shortcut, or syn.exe) and
  Excel installed. The console address is where that Syn answers.

  Usage:
    powershell -ExecutionPolicy Bypass -File scripts\live-layout.ps1 [-Url http://127.0.0.1:7777]
#>
[CmdletBinding()]
param([string]$Url = 'http://127.0.0.1:7777')

$ErrorActionPreference = 'Stop'

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class Win {
    [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rt, B; }
    [StructLayout(LayoutKind.Sequential)] public struct MI { public int Size; public R Monitor; public R Work; public int Flags; }
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
    [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h, uint f);
    [DllImport("user32.dll")] public static extern bool GetMonitorInfo(IntPtr m, ref MI i);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out R r, int s);
    public static R Drawn(IntPtr h) {
        R r; if (DwmGetWindowAttribute(h, 9, out r, 16) != 0) GetWindowRect(h, out r); return r;
    }
    public static R Raw(IntPtr h) { R r; GetWindowRect(h, out r); return r; }
    public static R Work(IntPtr h) {
        MI i = new MI(); i.Size = Marshal.SizeOf(typeof(MI));
        GetMonitorInfo(MonitorFromWindow(h, 2), ref i); return i.Work;
    }
}
'@
# The same pixels the console sees, not a scaled-down version of them.
[void][Win]::SetProcessDpiAwarenessContext([IntPtr](-4))

$script:failures = 0
function Verdict([string]$label, [bool]$ok, [string]$why) {
    if (-not $ok) { $script:failures++ }
    Write-Host ("{0}  {1,-58} {2}" -f $(if ($ok) { 'PASS' } else { 'FAIL' }), $label, $why) -ForegroundColor $(if ($ok) { 'Green' } else { 'Red' })
}

$synProc = Get-Process syn-window -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $synProc) { throw "no Syn window: start Syn from its shortcut first (a console in a browser tab cannot be moved)" }
$syn = $synProc.MainWindowHandle

# Excel: the running one if there is one, else one of this script's own. "Running" is only what COM can attach
# to: an Excel sitting at its start screen is not registered, so a new instance is made beside it, and that one
# (found by what is new in the process list) is this script's to end, whatever else is running.
$before = @(Get-Process EXCEL -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
$xl = $null
try { $xl = [Runtime.InteropServices.Marshal]::GetActiveObject('Excel.Application') } catch { }
if (-not $xl) { $xl = New-Object -ComObject Excel.Application }
$mine = @(Get-Process EXCEL -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id })
$ownExcel = $mine.Count -gt 0
$xl.Visible = $true
$book = $null
if ($ownExcel -or $xl.Workbooks.Count -eq 0) { $book = $xl.Workbooks.Add() }
$xlh = [IntPtr]$xl.Hwnd

# What Syn remembers of the layout, which the cases below overwrite: put back at the end.
$remembered = $null
try { $remembered = (& curl.exe -s "$Url/layout") -join '' | ConvertFrom-Json } catch { }

# Where everything is now, to put it back.
$was = @{}
foreach ($h in @($syn, $xlh)) {
    $was[[string]$h] = @{ Rect = [Win]::Raw($h); Max = [Win]::IsZoomed($h); Min = [Win]::IsIconic($h) }
}

function Ask([string]$body, [string]$origin = $Url) {
    $f = Join-Path $env:TEMP 'syn-layout-body.json'
    [IO.File]::WriteAllText($f, $body)
    $curlArgs = @('-s', '-w', "`n%{http_code}", '--data-binary', "@$f")
    if ($origin) { $curlArgs += @('-H', "Origin: $origin") }
    $out = & curl.exe @curlArgs "$Url/layout"
    $lines = @($out)
    [pscustomobject]@{ Code = [int]$lines[-1]; Body = ($lines[0..($lines.Count - 2)] -join "`n") }
}

function Place([string]$side, [double]$share, [string]$label) {
    $r = Ask ('{{"side":"{0}","share":{1}}}' -f $side, $share.ToString([Globalization.CultureInfo]::InvariantCulture))
    Start-Sleep -Milliseconds 700
    Verdict "$label : the console says it did it" ($r.Code -eq 200) "$($r.Code) $($r.Body)"
    $work = [Win]::Work($syn)
    $s = [Win]::Drawn($syn); $x = [Win]::Drawn($xlh)
    $workW = $work.Rt - $work.L
    $want = [math]::Round($workW * $share)
    $sw = $s.Rt - $s.L
    # Syn has a smallest width; then it keeps that, and Excel has what is left.
    $narrower = $sw -gt $want + 2
    Verdict "$label : Syn is the share asked for" (([math]::Abs($sw - $want) -le 2) -or $narrower) "Syn $sw px, asked for $want of $workW$(if ($narrower) { ' (its smallest width)' })"
    if ($side -eq 'right') {
        Verdict "$label : Syn is against the right edge" ([math]::Abs($s.Rt - $work.Rt) -le 1) "Syn right $($s.Rt), screen $($work.Rt)"
        Verdict "$label : Excel starts at the left edge" ([math]::Abs($x.L - $work.L) -le 1) "Excel left $($x.L), screen $($work.L)"
        Verdict "$label : Excel meets Syn, no gap, no overlap" ([math]::Abs($x.Rt - $s.L) -le 1) "Excel right $($x.Rt), Syn left $($s.L)"
    } else {
        Verdict "$label : Syn is against the left edge" ([math]::Abs($s.L - $work.L) -le 1) "Syn left $($s.L), screen $($work.L)"
        Verdict "$label : Excel ends at the right edge" ([math]::Abs($x.Rt - $work.Rt) -le 1) "Excel right $($x.Rt), screen $($work.Rt)"
        Verdict "$label : Excel meets Syn, no gap, no overlap" ([math]::Abs($x.L - $s.Rt) -le 1) "Excel left $($x.L), Syn right $($s.Rt)"
    }
    Verdict "$label : both fill the screen top to bottom" (
        ([math]::Abs($s.T - $work.T) -le 1) -and ([math]::Abs($s.B - $work.B) -le 1) -and
        ([math]::Abs($x.T - $work.T) -le 1) -and ([math]::Abs($x.B - $work.B) -le 1)) "Syn $($s.T)-$($s.B), Excel $($x.T)-$($x.B), screen $($work.T)-$($work.B)"
}

try {
    $g = (& curl.exe -s "$Url/layout") -join ''
    Verdict 'the console says Syn can be moved' ($g -match '"can":true') $g

    Place 'right' 0.3333 'right, a third'
    Place 'left' 0.5 'left, a half'
    Place 'right' 0.6667 'right, two thirds'
    Place 'left' 0.25 'left, a quarter'

    # A maximized Excel is brought back to the place; so is one in the taskbar.
    [void][Win]::ShowWindow($xlh, 3); Start-Sleep -Milliseconds 500
    Place 'right' 0.3333 'Excel maximized'
    Verdict 'Excel is no longer maximized' (-not [Win]::IsZoomed($xlh)) ''
    [void][Win]::ShowWindow($xlh, 6); Start-Sleep -Milliseconds 500
    Place 'right' 0.4 'Excel minimized'
    Verdict 'Excel is out of the taskbar' (-not [Win]::IsIconic($xlh)) ''

    # Refusals move nothing.
    $before = [Win]::Drawn($syn)
    $r = Ask '{"side":"right","share":0.9}'
    Verdict 'a share that is no layout is refused, with the way to ask' (($r.Code -eq 409) -and ($r.Body -match '0.2 to 0.8')) "$($r.Code) $($r.Body)"
    $r = Ask '{"side":"top","share":0.5}'
    Verdict 'a side that is not left or right is refused' ($r.Code -eq 409) "$($r.Code)"
    $r = Ask '{"side":"left","share":0.5,"monitor":2}'
    Verdict 'a key that is not in the schema is refused' (($r.Code -eq 409) -and ($r.Body -match 'no key')) "$($r.Code) $($r.Body)"
    $r = Ask '{"side":"left","share":0.5}' ''
    Verdict 'a request with no Origin is refused' ($r.Code -eq 403) "$($r.Code) $($r.Body)"
    $r = Ask '{"side":"left","share":0.5}' 'http://evil.example'
    Verdict 'a request from another page is refused' ($r.Code -eq 403) "$($r.Code) $($r.Body)"
    $after = [Win]::Drawn($syn)
    Verdict 'none of the refused requests moved Syn' (($before.L -eq $after.L) -and ($before.Rt -eq $after.Rt) -and ($before.T -eq $after.T)) "left $($before.L)->$($after.L)"

    # The screenshot is the proof a person would ask for.
    $shots = Join-Path (Split-Path -Parent $PSScriptRoot) 'testbed\shots'
    New-Item -ItemType Directory -Force $shots | Out-Null
    Place 'right' 0.3333 'the picture'
    Add-Type -AssemblyName System.Drawing
    $work = [Win]::Work($syn)
    $bmp = New-Object System.Drawing.Bitmap ($work.Rt - $work.L), ($work.B - $work.T)
    $gfx = [System.Drawing.Graphics]::FromImage($bmp)
    $gfx.CopyFromScreen($work.L, $work.T, 0, 0, $bmp.Size)
    $file = Join-Path $shots 'layout-right-third.png'
    $bmp.Save($file); $gfx.Dispose(); $bmp.Dispose()
    Write-Host "      screenshot: $file"
}
finally {
    # Syn's remembered choice first, while Excel is still there to be arranged with it.
    if ($remembered -and $remembered.side) {
        $sh = $remembered.share.ToString([Globalization.CultureInfo]::InvariantCulture)
        $null = Ask ('{{"side":"{0}","share":{1},"auto":{2}}}' -f $remembered.side, $sh, $remembered.auto.ToString().ToLower())
    }
    # Put it back: sizes, then the states they had.
    foreach ($h in @($syn, $xlh)) {
        $w = $was[[string]$h]
        if ([Win]::IsZoomed($h) -or [Win]::IsIconic($h)) { [void][Win]::ShowWindow($h, 9) }
        [void][Win]::SetWindowPos($h, [IntPtr]::Zero, $w.Rect.L, $w.Rect.T, $w.Rect.Rt - $w.Rect.L, $w.Rect.B - $w.Rect.T, 0x0014)
        if ($w.Max) { [void][Win]::ShowWindow($h, 3) } elseif ($w.Min) { [void][Win]::ShowWindow($h, 6) }
    }
    if ($book) { try { $book.Saved = $true; $book.Close($false) } catch { } }
    if ($ownExcel) {
        try { $xl.Quit() } catch { }
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($xl)
        $xl = $null; $book = $null
        [GC]::Collect(); [GC]::WaitForPendingFinalizers()
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ((Get-Process -Id $mine.Id -ErrorAction SilentlyContinue) -and $clock.Elapsed.TotalSeconds -lt 20) { Start-Sleep -Milliseconds 250 }
        # An Excel that was asked to quit, with nothing open, has been seen to stay on; it is this script's own and
        # empty, so it is ended rather than left behind.
        $stayed = [bool](Get-Process -Id $mine.Id -ErrorAction SilentlyContinue)
        if ($stayed) { Stop-Process -Id $mine.Id -Force -ErrorAction SilentlyContinue }
        Verdict 'the Excel this script started is gone' (-not (Get-Process -Id $mine.Id -ErrorAction SilentlyContinue)) $(if ($stayed) { 'it did not quit by itself in 20 s and was ended' } else { '' })
    }
}

Write-Host ''
Write-Host ("{0} failure(s)" -f $script:failures) -ForegroundColor $(if ($script:failures) { 'Red' } else { 'Green' })
exit $script:failures
