<#
  live-layout-start.ps1 -- what Syn does with the windows already on the screen when it starts.

  live-layout-auto.ps1 proves what happens when Syn opens a document. This
  proves the other half: you have Excel open, then you start Syn. Syn used to
  take its third and leave a maximized Excel full screen behind it, which hid
  a third of the workbook until the first `open`. Now, with "Arrange by
  itself" on, the Excel, Word or PowerPoint window on Syn's screen takes the
  rest.

  Cases, each with a fresh Excel opened the way a person opens it and Syn
  started from its own launcher (syn.exe). This script sets only where
  things START (a window the person maximized, minimized or left on another
  monitor); every move after that is Syn's:
    1. Excel maximized on Syn's screen: restored, and it takes the rest,
       meeting Syn, both filling the screen top to bottom.
    2. Excel minimized: left minimized (the person put it away).
    3. Two monitors only: Syn last left on the other monitor from a
       maximized Excel: Syn takes its slot on its own monitor and Excel,
       which is the person's arrangement, is not pulled across.

  Refuses to run if Excel or Syn is already running: it starts both, and an
  Excel of yours is not this script's to move or close. Ends only the Excel
  and Syn it started, and puts back the window position Syn remembers.

  Needs an installed (or otherwise built-in-place) Syn with Arrange by itself
  on, which is its default. Usage:
    powershell -ExecutionPolicy Bypass -File scripts\live-layout-start.ps1 [-Syn <path to syn.exe>]
  The exit code is the number of failures.
#>
[CmdletBinding()]
param([string]$Syn = (Join-Path $env:LOCALAPPDATA 'Programs\Syn\syn.exe'))

$ErrorActionPreference = 'Stop'
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class S {
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rt, B; }
  [StructLayout(LayoutKind.Sequential)] public struct MI { public int Size; public R Monitor; public R Work; public int Flags; }
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h, uint f);
  [DllImport("user32.dll")] public static extern bool GetMonitorInfo(IntPtr m, ref MI i);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out R r, int s);
  // What is drawn, without the invisible resize border.
  public static R Drawn(IntPtr h) { R r; if (DwmGetWindowAttribute(h, 9, out r, 16) != 0) GetWindowRect(h, out r); return r; }
  public static R Work(IntPtr h) { MI i = new MI(); i.Size = Marshal.SizeOf(typeof(MI)); GetMonitorInfo(MonitorFromWindow(h, 2), ref i); return i.Work; }
}
'@
[void][S]::SetProcessDpiAwarenessContext([IntPtr](-4))
Add-Type -AssemblyName System.Windows.Forms

$script:failures = 0
function Verdict([string]$label, [bool]$ok, [string]$why) {
    if (-not $ok) { $script:failures++ }
    Write-Host (("{0}  {1,-66} {2}" -f $(if ($ok) { 'PASS' } else { 'FAIL' }), $label, $why))
}
function Near($a, $b) { [math]::Abs($a - $b) -le 1 }

if (-not (Test-Path $Syn)) { throw "no Syn at $Syn" }
if (Get-Process EXCEL, syn-window, ui, cli -ErrorAction SilentlyContinue) {
    throw 'Excel or Syn is already running: close it first, this script starts both and does not touch one of yours.'
}
$agent = Join-Path (Split-Path -Parent $Syn) '.agent'
$layout = Join-Path $agent 'layout.json'
if ((Test-Path $layout) -and ((Get-Content $layout -Raw) -match '"auto"\s*:\s*false')) {
    throw 'Arrange by itself is off in this Syn: turn it on in the status menu, or delete layout.json.'
}
$windowTxt = Join-Path $agent 'window.txt'
$wasWindow = if (Test-Path $windowTxt) { Get-Content $windowTxt -Raw }

function Start-Excel {
    $x = $null
    $null = Start-Process excel.exe
    $until = (Get-Date).AddSeconds(40)
    do {
        Start-Sleep -Milliseconds 500
        $x = Get-Process EXCEL -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
    } while (-not $x -and (Get-Date) -lt $until)
    if (-not $x) { throw 'Excel did not open a window' }
    Start-Sleep -Seconds 3   # past its splash, onto the start screen
    $x.Refresh()
    $x
}

# Starts Syn from its launcher; returns its window process once it has one.
function Start-Syn {
    Start-Process $Syn
    $until = (Get-Date).AddSeconds(40)
    do {
        Start-Sleep -Milliseconds 400
        $w = Get-Process syn-window -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
    } while (-not $w -and (Get-Date) -lt $until)
    if (-not $w) { throw 'Syn did not open its window' }
    $w
}

# Ends only what a case started: Syn by closing its window, the Excel it opened by its own window.
function Stop-All($xl) {
    Get-Process syn-window -ErrorAction SilentlyContinue | ForEach-Object { [void]$_.CloseMainWindow() }
    $clock = [Diagnostics.Stopwatch]::StartNew()
    while ((Get-Process syn-window, ui, cli, office-host -ErrorAction SilentlyContinue) -and $clock.Elapsed.TotalSeconds -lt 30) { Start-Sleep -Milliseconds 300 }
    $p = Get-Process -Id $xl.Id -ErrorAction SilentlyContinue
    if ($p) {
        [void]$p.CloseMainWindow()
        if (-not $p.WaitForExit(15000)) { $p.Kill() }
    }
}

# A Syn window position as the window program remembers it: x,y,width,height,maximized.
function Leave-Syn([string]$rect) { Set-Content $windowTxt $rect -Encoding ascii -NoNewline }

$xl = $null
try {
    # --- 1. Excel maximized on Syn's screen --------------------------------
    Leave-Syn '1273,0,654,1039,0'
    $xl = Start-Excel
    [void][S]::ShowWindow($xl.MainWindowHandle, 3)
    Start-Sleep -Seconds 1
    Verdict 'set up: Excel is maximized' ([S]::IsZoomed($xl.MainWindowHandle)) ''
    $w = Start-Syn
    Start-Sleep -Seconds 6
    $s = [S]::Drawn($w.MainWindowHandle); $e = [S]::Drawn($xl.MainWindowHandle); $work = [S]::Work($w.MainWindowHandle)
    $third = [math]::Round(($work.Rt - $work.L) / 3)
    Verdict 'Syn takes its third of the screen by itself' ((Near $s.L ($work.Rt - $third)) -and (Near $s.Rt $work.Rt)) "Syn $($s.L)-$($s.Rt), screen $($work.L)-$($work.Rt)"
    Verdict 'the maximized Excel is restored and takes the rest' ((-not [S]::IsZoomed($xl.MainWindowHandle)) -and (Near $e.L $work.L) -and (Near $e.Rt $s.L)) "Excel $($e.L)-$($e.Rt), Syn $($s.L)-$($s.Rt)"
    Verdict 'Excel and Syn fill the screen top to bottom' ((Near $e.T $work.T) -and (Near $e.B $work.B) -and (Near $s.T $work.T) -and (Near $s.B $work.B)) "Excel $($e.T)-$($e.B), Syn $($s.T)-$($s.B), screen $($work.T)-$($work.B)"
    Stop-All $xl; $xl = $null

    # --- 2. Excel minimized: the person put it away ------------------------
    Leave-Syn '1273,0,654,1039,0'
    $xl = Start-Excel
    [void][S]::ShowWindow($xl.MainWindowHandle, 6)
    Start-Sleep -Seconds 1
    $w = Start-Syn
    Start-Sleep -Seconds 6
    $s = [S]::Drawn($w.MainWindowHandle); $work = [S]::Work($w.MainWindowHandle)
    Verdict 'Syn still takes its place with Excel minimized' ((Near $s.L ($work.Rt - $third)) -and (Near $s.Rt $work.Rt)) "Syn $($s.L)-$($s.Rt)"
    Verdict 'a minimized Excel is left minimized' ([S]::IsIconic($xl.MainWindowHandle)) ''
    Stop-All $xl; $xl = $null

    # --- 3. two monitors: Excel on one, Syn left on the other -------------
    if ([Windows.Forms.Screen]::AllScreens.Count -lt 2) {
        Write-Host 'skip  case 3 needs a second monitor'
    }
    else {
        # Excel opens where it was last used; Syn is left on whichever monitor Excel is not on.
        $xl = Start-Excel
        [void][S]::ShowWindow($xl.MainWindowHandle, 3)
        Start-Sleep -Seconds 1
        $xw = [S]::Work($xl.MainWindowHandle)
        $o = ([Windows.Forms.Screen]::AllScreens | Where-Object { $_.WorkingArea.Left -ne $xw.L -or $_.WorkingArea.Top -ne $xw.T } | Select-Object -First 1).WorkingArea
        Leave-Syn ("{0},{1},900,700,0" -f ($o.Left + 100), ($o.Top + 100))
        $before = [S]::Drawn($xl.MainWindowHandle)
        $w = Start-Syn
        Start-Sleep -Seconds 6
        $s = [S]::Drawn($w.MainWindowHandle); $e = [S]::Drawn($xl.MainWindowHandle); $work = [S]::Work($w.MainWindowHandle)
        Verdict 'Syn takes its slot on the monitor it was left on' ((Near $s.Rt $work.Rt) -and (Near $work.L $o.Left) -and ($s.L -ge $o.Left)) "Syn $($s.L)-$($s.Rt), that monitor $($o.Left)-$($o.Right)"
        Verdict 'an Excel on the other monitor is not pulled across' ([S]::IsZoomed($xl.MainWindowHandle) -and (Near $e.L $before.L) -and (Near $e.Rt $before.Rt)) "Excel $($e.L)-$($e.Rt), was $($before.L)-$($before.Rt)"
        Stop-All $xl; $xl = $null
    }
}
finally {
    if ($xl) { Stop-All $xl }
    if ($null -ne $wasWindow) { Set-Content $windowTxt $wasWindow -Encoding ascii -NoNewline }
    Verdict 'nothing of Syn or Excel is left running' (-not (Get-Process EXCEL, syn-window, ui, cli, office-host -ErrorAction SilentlyContinue)) ''
}

Write-Host ''
Write-Host ("{0} failure(s)." -f $script:failures) -ForegroundColor $(if ($script:failures) { 'Red' } else { 'Green' })
exit $script:failures
