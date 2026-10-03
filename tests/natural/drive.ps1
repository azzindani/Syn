# Send a tests/natural script to the console, one prompt per turn, each only
# after the last has finished (a `say` through /cmd returns when the turn
# ends). Logs every turn: the prompt, how long it took, and how it ended.
# The console must already be running (scripts/console.ps1). -Model is the
# console's `model` command argument; leave it out to use what is configured.
#   powershell -File tests/natural/drive.ps1 -Script tests/natural/oil.md `
#       -Log testbed/oil.log -Workspace C:\path	oolder\with	he\data
# score.py then reads the log and the per-turn files beside it.
param([string]$Script, [string]$Log, [string]$Workspace, [string]$Model = '', [int]$Wait = 30, [int]$From = 1)
$ErrorActionPreference = 'Stop'
$prompts = Select-String -Path $Script -Pattern '^(\d+)\. (.+)$' | ForEach-Object { [pscustomobject]@{ N = [int]$_.Matches[0].Groups[1].Value; Text = $_.Matches[0].Groups[2].Value } }
$h = @{ Origin = 'http://127.0.0.1:7777' }
function Say($line) {
    $body = [Text.Encoding]::UTF8.GetBytes($line)
    $r = Invoke-WebRequest http://127.0.0.1:7777/cmd -Method Post -Headers $h -Body $body -ContentType 'text/plain; charset=utf-8' -UseBasicParsing -TimeoutSec 7200
    $bytes = $r.RawContentStream.ToArray()
    return ([Text.Encoding]::UTF8.GetString($bytes) | ConvertFrom-Json).out
}
"[$(Get-Date -Format HH:mm:ss)] waiting $Wait s before the first prompt" | Add-Content $Log -Encoding utf8
Start-Sleep -Seconds $Wait
# The session is set up after the wait, not before it: a page reloading onto
# a restarted console reopens the chat in its address bar, and that switched
# the console to an old chat with no workspace when this ran first.
$chat = ''
if ($From -eq 1) {
    $out = Say 'chat new'
    $chat = [regex]::Match($out, 'RECEIPT chat=(\S+)').Groups[1].Value
    if ($Model) { $null = Say "model $Model" }
    $ws = Say "workspace $Workspace"
    "[$(Get-Date -Format HH:mm:ss)] SESSION chat=$chat $((($ws -split "`n") | Where-Object { $_ -match 'RECEIPT workspace' }))" | Add-Content $Log -Encoding utf8
    if (-not $chat) { "[$(Get-Date -Format HH:mm:ss)] CONSOLE ERROR: no new chat" | Add-Content $Log -Encoding utf8; exit 1 }
}
foreach ($p in $prompts | Where-Object { $_.N -ge $From }) {
    "[$(Get-Date -Format HH:mm:ss)] TURN $($p.N) SENT: $($p.Text)" | Add-Content $Log -Encoding utf8
    $t = Get-Date
    try { $out = Say ("say " + $p.Text) } catch { "[$(Get-Date -Format HH:mm:ss)] TURN $($p.N) CONSOLE ERROR: $($_.Exception.Message)" | Add-Content $Log -Encoding utf8; break }
    # Every turn in the session's own chat, or stop: carrying on in another
    # chat is a different conversation, and the run would mean nothing.
    $ran = [regex]::Match($out, 'RECEIPT say \S+ \S+ chat=(\S+)').Groups[1].Value
    if ($chat -and $ran -and $ran -ne $chat) {
        "[$(Get-Date -Format HH:mm:ss)] TURN $($p.N) CONSOLE ERROR: ran in chat $ran, not the session's $chat; stopped" | Add-Content $Log -Encoding utf8
        break
    }
    $secs = [int]((Get-Date) - $t).TotalSeconds
    $lines = $out -split "`n"
    $steps = ($lines | Where-Object { $_ -match '^RECEIPT step ' }).Count
    $end = $lines | Where-Object { $_ -match '^(ANSWER|STOPPED|ERROR) ' } | Select-Object -Last 1
    if (-not $end) { $end = '(no answer line)' }
    "[$(Get-Date -Format HH:mm:ss)] TURN $($p.N) DONE in ${secs}s, $steps step(s): $end" | Add-Content $Log -Encoding utf8
    Set-Content -Path ($Log -replace '\.log$', "-turn$($p.N).txt") -Value $out -Encoding utf8
}
"[$(Get-Date -Format HH:mm:ss)] SCRIPT FINISHED" | Add-Content $Log -Encoding utf8
