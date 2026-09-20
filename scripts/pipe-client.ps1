<#
  pipe-client.ps1 — send office-rpc/1 envelopes to a running office-host.

  The sidecar speaks one JSON object per line over a named pipe. Nothing in
  the Rust core can talk to it yet (see mcpgate::rpc_request, which builds the
  envelope and has no transport), so this is the only client that exists and
  the way the Windows runbook's smoke tests are driven.

  Usage:
    scripts\pipe-client.ps1 -Pipe hand-excel -Json '{"method":"read",...}'
    scripts\pipe-client.ps1 -Pipe hand-excel -File testbed\rpc\reads.jsonl

  Every request and reply is echoed and, with -Log, appended to a transcript.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Pipe,
    [string[]]$Json,
    [string]$File,
    [string]$Log,
    [int]$TimeoutMs = 20000
)

$ErrorActionPreference = 'Stop'

$lines = @()
if ($Json) { $lines += $Json }
if ($File) { $lines += (Get-Content -LiteralPath $File | Where-Object { $_.Trim() -and -not $_.StartsWith('#') }) }
if (-not $lines) { throw 'nothing to send: pass -Json or -File' }

$client = New-Object System.IO.Pipes.NamedPipeClientStream('.', $Pipe, [System.IO.Pipes.PipeDirection]::InOut)
try {
    $client.Connect($TimeoutMs)
}
catch {
    throw "could not connect to pipe '$Pipe' within ${TimeoutMs}ms. Is office-host running with --pipe $Pipe ?"
}

# UTF8Encoding($false): a BOM from the stock encoding would be prepended to
# the first request on the wire.
$utf8 = New-Object System.Text.UTF8Encoding($false)
$reader = New-Object System.IO.StreamReader($client, $utf8, $false)
$writer = New-Object System.IO.StreamWriter($client, $utf8)
$writer.AutoFlush = $true

$transcript = @()
try {
    foreach ($line in $lines) {
        Write-Host ">> $line" -ForegroundColor DarkGray
        $writer.WriteLine($line)

        # ReadLine blocks forever if the sidecar wedged on a modal dialog, so
        # read on a worker and give up rather than inheriting the hang.
        $task = $reader.ReadLineAsync()
        if (-not $task.Wait($TimeoutMs)) {
            $reply = '{"ok":false,"error":"CLIENT TIMEOUT: no reply (sidecar wedged?)"}'
        }
        else {
            $reply = $task.Result
        }
        Write-Host "<< $reply"
        $transcript += "$line`t$reply"
    }
}
finally {
    foreach ($d in $reader, $writer, $client) { if ($d) { try { $d.Dispose() } catch { } } }
}

if ($Log) {
    New-Item -ItemType Directory -Force -Path (Split-Path $Log) | Out-Null
    Add-Content -LiteralPath $Log -Value $transcript -Encoding utf8
}
