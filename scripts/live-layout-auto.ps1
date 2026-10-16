<#
  live-layout-auto.ps1 -- Syn and the apps it opens take their places by themselves.

  live-layout.ps1 proves what the menu's buttons do. This proves the part
  nobody clicks: with "Arrange by itself" on, Syn goes to its place and the
  Excel, Word and PowerPoint it opens go beside it, with no request from the
  page; with it off, they are left where they open; and a window the person
  moves afterwards stays where they put it.

  Cases, each through the real mcpgate.exe (the same `open` Syn's loop runs):
    1. Automatic off: Syn opens a workbook, and Excel is not moved.
    2. Automatic on: Syn takes its place on its own (turning it on), then
       opens a workbook, a document and a deck, and each takes the rest,
       edge to edge, with no POST from this script in between.
    3. A person moves Excel, Syn opens the workbook again: Excel stays where
       the person put it.

  Needs Syn running in a window of its own, set (in its menu) to the right
  and 33%, which is what it is until someone changes it: this script does
  not move windows to set that up, because with no Office app open a side
  and a share put whatever window you were in beside Syn. An Office app that
  is already running is left alone, and so is whatever it holds: only the
  copies of plan.xlsx, report.docx and deck.pptx under testbed\docs
  (new-testbed-docs.ps1 makes them) are opened, found by their window
  titles, and closed again; an app this script started is ended. Turning
  automatic on does move an Office window that is already open into place,
  which is what it is for. What was set for "Arrange by itself" is put back.
  The exit code is the number of failures.

  Build first:
    cd core; cargo build --release; cd ..
    dotnet build -c Release sidecar-csharp\Host
  and have Syn running from a build of the same commit (the console answers
  /layout from the program that is running).

  Usage:
    powershell -ExecutionPolicy Bypass -File scripts\live-layout-auto.ps1 [-Url http://127.0.0.1:7777] [-AgentHome <the running Syn's .agent folder>]
#>
[CmdletBinding()]
param([string]$Url = 'http://127.0.0.1:7777', [string]$AgentHome = '')

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$docs = Join-Path $root 'testbed\docs'

$gate = @('core\target\release\mcpgate.exe', 'core\target\debug\mcpgate.exe') |
    ForEach-Object { Join-Path $root $_ } | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $gate) { throw "mcpgate.exe not found: cd core; cargo build --release" }
$officeHost = Join-Path $root 'sidecar-csharp\Host\bin\Release\net8.0-windows\office-host.exe'
if (-not (Test-Path $officeHost)) { throw "office-host.exe not found: dotnet build -c Release sidecar-csharp\Host" }

# What each application is called to Syn, to Windows, and to COM.
$apps = @(
    @{ App = 'excel';      Cls = 'XLMAIN';        File = 'plan.xlsx';   Proc = 'EXCEL';    Prog = 'Excel.Application';      Docs = 'Workbooks' },
    @{ App = 'word';       Cls = 'OpusApp';       File = 'report.docx'; Proc = 'WINWORD';  Prog = 'Word.Application';       Docs = 'Documents' },
    @{ App = 'powerpoint'; Cls = 'PPTFrameClass'; File = 'deck.pptx';   Proc = 'POWERPNT'; Prog = 'PowerPoint.Application'; Docs = 'Presentations' }
)
foreach ($a in $apps) {
    if (-not (Test-Path (Join-Path $docs $a.File))) { throw "missing $($a.File) under $docs -- run scripts\new-testbed-docs.ps1 first" }
    $a.Own = -not (Get-Process $a.Proc -ErrorAction SilentlyContinue)
}

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class Win {
    [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rt, B; }
    [StructLayout(LayoutKind.Sequential)] public struct MI { public int Size; public R Monitor; public R Work; public int Flags; }
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h, uint f);
    [DllImport("user32.dll")] public static extern bool GetMonitorInfo(IntPtr m, ref MI i);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out R r, int s);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr p);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, System.Text.StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, System.Text.StringBuilder s, int n);
    delegate bool EnumProc(IntPtr h, IntPtr p);
    // The visible top-level windows of a class whose title contains `part`, topmost first.
    public static IntPtr[] Find(string cls, string part) {
        var found = new System.Collections.Generic.List<IntPtr>();
        EnumWindows((h, p) => {
            if (IsWindowVisible(h)) {
                var c = new System.Text.StringBuilder(256); var t = new System.Text.StringBuilder(512);
                GetClassName(h, c, 256); GetWindowText(h, t, 512);
                if (c.ToString() == cls && t.ToString().IndexOf(part, StringComparison.OrdinalIgnoreCase) >= 0) found.Add(h);
            }
            return true;
        }, IntPtr.Zero);
        return found.ToArray();
    }
    public static R Drawn(IntPtr h) { R r; if (DwmGetWindowAttribute(h, 9, out r, 16) != 0) GetWindowRect(h, out r); return r; }
    public static R Raw(IntPtr h) { R r; GetWindowRect(h, out r); return r; }
    public static R Work(IntPtr h) { MI i = new MI(); i.Size = Marshal.SizeOf(typeof(MI)); GetMonitorInfo(MonitorFromWindow(h, 2), ref i); return i.Work; }
}
'@
[void][Win]::SetProcessDpiAwarenessContext([IntPtr](-4))

$script:failures = 0
function Verdict([string]$label, [bool]$ok, [string]$why) {
    if (-not $ok) { $script:failures++ }
    Write-Host ("{0}  {1,-66} {2}" -f $(if ($ok) { 'PASS' } else { 'FAIL' }), $label, $why) -ForegroundColor $(if ($ok) { 'Green' } else { 'Red' })
}

$synProc = Get-Process syn-window -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $synProc) { throw "no Syn window: start Syn from its shortcut first" }
$syn = $synProc.MainWindowHandle
# The folder the running Syn keeps its choice in, which mcpgate must read too: the one AGENT_HOME names, else
# `.agent` beside the .env (AGENT_ENV_FILE), else `.agent` where Syn is installed (its launcher starts there).
if (-not $AgentHome) {
    $AgentHome = if ($env:AGENT_HOME) { $env:AGENT_HOME }
                 elseif ($env:AGENT_ENV_FILE) { Join-Path (Split-Path -Parent $env:AGENT_ENV_FILE) '.agent' }
                 else { Join-Path (Split-Path -Parent $synProc.Path) '.agent' }
}

function Ask([string]$body) {
    $f = Join-Path $env:TEMP 'syn-layout-auto-body.json'
    [IO.File]::WriteAllText($f, $body)
    $out = & curl.exe -s -w "`n%{http_code}" -H "Origin: $Url" --data-binary "@$f" "$Url/layout"
    $lines = @($out)
    [pscustomobject]@{ Code = [int]$lines[-1]; Body = ($lines[0..($lines.Count - 2)] -join "`n") }
}
function State { (& curl.exe -s "$Url/layout") -join '' | ConvertFrom-Json }

$orig = State
if (-not $orig.can) { throw "the console says Syn cannot be moved: $($orig | ConvertTo-Json -Compress)" }
if (($orig.side -ne 'right') -or ([math]::Abs($orig.share - 0.3333) -gt 0.001)) {
    throw "set Syn's menu to Right and 33% first (it is $($orig.side) $($orig.share)): this script does not move windows to set that"
}

# Where Syn and the rest of the screen should be at Right and a third.
function Slot {
    $work = [Win]::Work($syn)
    $w = [math]::Round(($work.Rt - $work.L) * 0.3333)
    [pscustomobject]@{ Work = $work; SynLeft = $work.Rt - $w; AppRight = $work.Rt - $w }
}
function Near($a, $b) { [math]::Abs($a - $b) -le 1 }

# --- the same plumbing live-helper-exit.ps1 uses ---------------------------
function Start-Gate {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $gate
    $psi.WorkingDirectory = $root
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.CreateNoWindow = $true
    $psi.EnvironmentVariables['AGENT_PIPE_EXCEL'] = 'auto-excel'
    $psi.EnvironmentVariables['AGENT_PIPE_WORD'] = 'auto-word'
    $psi.EnvironmentVariables['AGENT_PIPE_PPT'] = 'auto-ppt'
    $psi.EnvironmentVariables['AGENT_OFFICE_HOST'] = $officeHost
    $psi.EnvironmentVariables['AGENT_MCP_ROOTS'] = $docs
    # The same data folder as the running Syn, so the choice made in its menu is the one read here.
    $psi.EnvironmentVariables['AGENT_HOME'] = $AgentHome
    $script:id = 0
    $p = [System.Diagnostics.Process]::Start($psi)
    $null = Invoke-Rpc $p 'initialize' @{ protocolVersion = '2025-11-25'; capabilities = @{}; clientInfo = @{ name = 'live-layout-auto'; version = '1' } }
    return $p
}
function Invoke-Rpc($p, [string]$method, $params) {
    $script:id++
    $msg = @{ jsonrpc = '2.0'; id = $script:id; method = $method; params = $params } | ConvertTo-Json -Compress -Depth 10
    $p.StandardInput.WriteLine($msg); $p.StandardInput.Flush()
    $line = $p.StandardOutput.ReadLine()
    if ($null -eq $line) { throw "mcpgate closed its output" }
    return $line | ConvertFrom-Json
}
function Call($p, [string]$tool, $arguments) {
    $r = Invoke-Rpc $p 'tools/call' @{ name = $tool; arguments = $arguments }
    if (-not $r.result -or $r.result.isError) { throw "$tool failed: $($r.result.content[0].text)" }
    return [string]$r.result.content[0].text
}
function Stop-Gate($p) { $p.StandardInput.Close(); if (-not $p.WaitForExit(30000)) { $p.Kill() } }

# The window of one of the testbed documents as drawn, once it has one. Found by class and title: an app that
# is already running keeps its other windows, and they are not this script's to measure.
function AppDrawn($a) {
    $h = [Win]::Find($a.Cls, $a.File) | Select-Object -First 1
    if ($h) { return [pscustomobject]@{ H = $h; R = [Win]::Drawn($h) } }
}
# Waits until the window is where Syn should put it (or the time is up); the last position seen.
function Wait-InSlot($a, [int]$seconds) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    do {
        $slot = Slot; $e = AppDrawn $a
        # Top and bottom too: an application puts its own size back once its first document is up, a moment
        # after Syn has placed it, and Syn puts it right again (layout.rs `hold`).
        if ($e -and (Near $e.R.Rt $slot.AppRight) -and (Near $e.R.L $slot.Work.L) -and (Near $e.R.T $slot.Work.T) -and (Near $e.R.B $slot.Work.B)) { return $e }
        Start-Sleep -Milliseconds 300
    } while ($clock.Elapsed.TotalSeconds -lt $seconds)
    return (AppDrawn $a)
}
# Closes the testbed document this script opened, and ends the app only if this script started it.
function Clear-App($a) {
    if (-not (Get-Process $a.Proc -ErrorAction SilentlyContinue)) { return }
    try {
        $app = [Runtime.InteropServices.Marshal]::GetActiveObject($a.Prog)
        foreach ($d in @($app.($a.Docs))) {
            if (([string]$d.FullName).StartsWith($docs, [StringComparison]::OrdinalIgnoreCase)) {
                try { $d.Saved = $true } catch { }
                if ($a.App -eq 'excel') { $d.Close($false) } elseif ($a.App -eq 'word') { $d.Close(0) } else { $d.Close() }
            }
        }
        $d = $null
        if ($a.Own) { if ($a.App -eq 'excel') { $app.DisplayAlerts = $false }; $app.Quit() }
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app); $app = $null
        [GC]::Collect(); [GC]::WaitForPendingFinalizers()
    } catch { }
    if ($a.Own) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ((Get-Process $a.Proc -ErrorAction SilentlyContinue) -and $clock.Elapsed.TotalSeconds -lt 30) { Start-Sleep -Milliseconds 300 }
    }
}

$excel = $apps[0]
$synWas = [Win]::Raw($syn)
try {
    # --- 1. automatic off: the app is left where it opens -------------------
    $r = Ask '{"auto":false}'
    Verdict 'automatic can be turned off' (($r.Code -eq 200) -and ((State).auto -eq $false)) "$($r.Code) $($r.Body)"
    $p = Start-Gate
    $null = Call $p 'open' @{ app = 'excel'; path = (Join-Path $docs $excel.File) }
    Start-Sleep -Seconds 6
    # Excel opens where it was last closed, which after a run with automatic on is Syn's slot, so "where it
    # opened" proves nothing. The person moves it, Syn is asked to open it again, and it must stay moved.
    $e = AppDrawn $excel
    if ($e) { [void][Win]::SetWindowPos($e.H, [IntPtr]::Zero, 120, 120, 900, 600, 0x0014) }   # (automatic is off here: nothing holds it)
    Start-Sleep -Milliseconds 500
    $null = Call $p 'open' @{ app = 'excel'; path = (Join-Path $docs $excel.File) }
    Start-Sleep -Seconds 6
    $e = AppDrawn $excel
    $raw = if ($e) { [Win]::Raw($e.H) }
    Verdict 'off: Excel is left where the person put it' ($e -and (Near $raw.L 120) -and (Near $raw.T 120)) $(if ($e) { "Excel's window at $($raw.L),$($raw.T), put at 120,120" } else { 'no Excel window' })
    Stop-Gate $p
    Clear-App $excel

    # --- 2. automatic on: Syn takes its place, then each app takes the rest -
    $r = Ask '{"auto":true}'
    Verdict 'automatic can be turned on' (($r.Code -eq 200) -and ((State).auto -eq $true)) "$($r.Code) $($r.Body)"
    Start-Sleep -Milliseconds 800
    $slot = Slot; $s = [Win]::Drawn($syn)
    Verdict 'on: Syn is in its place' ((Near $s.L $slot.SynLeft) -and (Near $s.Rt $slot.Work.Rt)) "Syn $($s.L)-$($s.Rt), expected $($slot.SynLeft)-$($slot.Work.Rt)"

    $p = Start-Gate
    $kept = $null
    foreach ($a in $apps) {
        $null = Call $p 'open' @{ app = $a.App; path = (Join-Path $docs $a.File) }
        $e = Wait-InSlot $a 15
        $slot = Slot; $s = [Win]::Drawn($syn)
        $name = $a.App
        Verdict "on: $name opened by Syn takes the rest, with no request from the page" ($e -and (Near $e.R.L $slot.Work.L) -and (Near $e.R.Rt $slot.AppRight)) $(if ($e) { "$name $($e.R.L)-$($e.R.Rt), Syn $($s.L)-$($s.Rt), screen $($slot.Work.L)-$($slot.Work.Rt)" } else { "no $name window" })
        Verdict "on: $name meets Syn, no gap, no overlap" ($e -and (Near $e.R.Rt $s.L)) $(if ($e) { "$name right $($e.R.Rt), Syn left $($s.L)" } else { '' })
        Verdict "on: $name and Syn fill the screen top to bottom" ($e -and (Near $e.R.T $slot.Work.T) -and (Near $e.R.B $slot.Work.B) -and (Near $s.T $slot.Work.T) -and (Near $s.B $slot.Work.B)) $(if ($e) { "$name $($e.R.T)-$($e.R.B), Syn $($s.T)-$($s.B), screen $($slot.Work.T)-$($slot.Work.B)" } else { '' })
        if ($a -eq $excel) { $kept = $e }
    }

    # --- 3. the person moves it; Syn does not take it back ------------------
    if ($kept) {
        # A person does not drag a window in the first seconds after Syn places it, and for those seconds Syn puts
        # back a window its application moves (layout.rs `hold`): wait them out before acting as the person.
        Start-Sleep -Seconds 4
        [void][Win]::SetWindowPos($kept.H, [IntPtr]::Zero, 120, 120, 900, 600, 0x0014)
        Start-Sleep -Milliseconds 500
        $null = Call $p 'open' @{ app = 'excel'; path = (Join-Path $docs $excel.File) }
        Start-Sleep -Seconds 5
        $e2 = AppDrawn $excel
        # SetWindowPos took the window's own edge (120, 120), which is 7 px outside what is drawn on the left.
        $raw = if ($e2) { [Win]::Raw($e2.H) }
        Verdict 'a window the person moved stays where they put it' ($e2 -and (Near $raw.L 120) -and (Near $raw.T 120)) $(if ($e2) { "Excel's window at $($raw.L),$($raw.T), put at 120,120" } else { 'no Excel window' })
    }
    Stop-Gate $p
    foreach ($a in $apps) { Clear-App $a }
}
finally {
    # Put back what was set, and Syn where it was.
    try { $null = Ask ('{"auto":' + $orig.auto.ToString().ToLower() + '}') } catch { }
    [void][Win]::SetWindowPos($syn, [IntPtr]::Zero, $synWas.L, $synWas.T, $synWas.Rt - $synWas.L, $synWas.B - $synWas.T, 0x0014)
    foreach ($a in $apps) {
        Clear-App $a
        Verdict "the testbed $($a.App) document is closed" (-not [Win]::Find($a.Cls, $a.File)) ''
        if ($a.Own) { Verdict "the $($a.App) this script started is gone" (-not (Get-Process $a.Proc -ErrorAction SilentlyContinue)) '' }
    }
    Verdict 'no office-host is left' (-not (Get-Process office-host -ErrorAction SilentlyContinue)) ''
}

Write-Host ''
Write-Host ("{0} failure(s)." -f $script:failures) -ForegroundColor $(if ($script:failures) { 'Red' } else { 'Green' })
exit $script:failures
