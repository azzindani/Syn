# Syn — one brain, many hands

Single-user desktop orchestrator: a small movable widget drives many open
software hands (Office first) in one live session, any model via OpenRouter.

## Layout
- `protocol/` — rpc catalog + default-deny security policy (JSON, language-neutral).
- `relay/` — Python POC of the relay core (stdlib only).
- `core/` — production Rust core (`core`, zero deps):
  - `protocol/bus/ops/security` — op vocabulary, relay, dispatch, policy.
  - `sessions` — multi-session hub (M5): independent runners, list/drop.
  - `memory` — session facts + cross-app transfer log + compaction (M2/M3).
  - `guard` — runtime allowlist + kill switch, enforced in `Runner::pump`.
  - `provider` — OpenRouter bodies, SSE streaming parse, Picker + budget.
  - `mcpgate` — MCP gateway: 6 ops as MCP tools over stdio.
  - `router/queue/runner/paths/stream/snapshots/acp/vfs/ooxml` — as before.
  - `agent` — the loop: a goal becomes tool calls, one per step, each
    dispatched through `Runner` so it meets every gate. `Brain` is a seam,
    so the whole loop is tested offline.
  - `tools` — the 7 tools the model may call (the 6 ops + `shell`), closed
    schemas, tool-call parsing, and a fingerprint of the surface.
  - `shell` — run ONE program: allowlisted, no shell metacharacters, no
    paths, deadline, capped output returned fenced as untrusted.
  - `hand` — `office-rpc/1` transport to a live sidecar (the brain's
    connection to the hands; generic over the stream, so testable anywhere).
  - `cdp` — Chrome DevTools hand: every browser AND every Electron app
    (VS Code, Slack, Figma, Notion) with no plugin. Selectors are bound as
    JS string literals, never pasted into source.
  - `ws` — RFC 6455 client (handshake, masking, ping, continuation) in std,
    because `core` has no dependencies.
  - bins: `cli` (REPL: `do`/`approve`/`deny`, `hand`/`live`, `shellallow`),
    `mcpgate` (stdio bridge).
- `widget/index.html` — the chat app, served by the `ui` binary: thread list,
  conversation, composer. It posts one command line to a loopback server
  that types it into a real `cli` process, so the page can only do what the
  CLI can do. Layout digested from t3code (`docs/DIGEST-05-t3code-ui.md`).
  (Tauri shell in `widget/src-tauri`, still unbuilt.)
  - `chats` — saved conversations as JSON Lines under `.syn/chats`
    (gitignored). One file per thread, appended per turn.
- `sidecar-csharp/Host/` — full STA COM sidecar (Word/Excel/PowerPoint,
  named-pipe office-rpc/1, timeout-guarded calls, .bak snapshots).
  Needs Windows + .NET 8 + Office to compile/run.
- `sidecar-csharp/Uia/` — the UI Automation sidecar (`uia-host`): reads any
  native window's control tree and presses controls by identity. Same
  office-rpc/1 wire, so core reaches it through the same `Hand`. MTA, not
  STA — a UIA client must not be STA or it can deadlock against an STA
  provider, which is the opposite of what the Office sidecar needs.
- `office-pane/` — Office.js task pane (Mac/Web hand), sideload to verify.
- `scripts/` — Windows bring-up: `new-testbed-docs.ps1` (fixtures via COM),
  `live-excel-smoke.ps1` (M1/M2 acceptance vs real Excel), `pipe-client.ps1`,
  `live-cdp-smoke.ps1` (a real browser), `live-uia-smoke.ps1` (Calculator).
- `testbed/` — gitignored run area for live Office tests (docs/out/logs).
- `.env.example` — provider key, endpoint, and the four model slots
  (`SYN_MODEL_LUNA|TERRA|SOL|ASTRA`); copy to `.env`, which is gitignored.
- `docs/` — 00→08, DIGEST-00→04, PRD, ideas, runbook-windows.
- `.tmp/repos/` — 9 cloned sources this was ported from.

## Port map (digested in docs/DIGEST-*)
- opencode loops -> bus events, part states, compaction budgets, retry,
  doom-loop gate, snapshot patch/revert, permission ask.
- dcc-mcp-office -> office-rpc/1 methods, two-layer default-deny, job phases,
  single STA write queue per sidecar (= per-file mutex).
- ai-office-mcp -> dual COM/openpyxl backends (= live/headless duality),
  SnapshotUndo stack. mcp-office -> env allowlist gates. word-mcp-server ->
  status enum, auto-.bak, Manager API. office-agents -> pane+bridge BYOK shape.
  gawirable -> LibreOffice fallback/export path.

## Run here (Linux POC)
```
python3 -m unittest discover -s tests   # 23 green
cd core && cargo test                      # 178 green
cargo run --example demo && ./target/release/cli < ../tests/e2e_script.txt
./target/release/cli < ../tests/e2e_safety.txt   # allow/kill/journal/replay/sessions
printf '{"tool":"list"}\n' | cargo run -q --bin mcpgate
```
## Run on Windows (live Office)
```
dotnet build sidecar-csharp\Host\Host.csproj -c Release
powershell -File scripts
ew-testbed-docs.ps1     # fixtures (close Office first)
powershell -File scripts\live-excel-smoke.ps1     # attach + read/write live Excel
```
Verified on Windows 11 + Microsoft 365 + .NET 8: the sidecar attaches to an
open workbook, reads and writes it live, fails closed on bad handles, and
detaches without closing the user's Excel — driven both by the PowerShell
smoke test and by `cli.exe` itself over the pipe (`hand` / `lread` / `lwrite`). Word/PowerPoint dispatch, export,
the Tauri bundle and a Rust-side pipe client are still unproven — status and
the COM lessons behind the fixes are in `docs/runbook-windows.md`.

Copy `.env.example` to `.env` and add a key to enable `send` and `do`.

## Many hands, one session

`Runner` holds several hands at once and routes each handle to the one that
claims its app, so the caller names a document and never a transport. A
named claim beats a catch-all; one dead transport drops only its own hand.

```
cli.exe
  hand synhand-excel excel word ppt   # COM sidecar, Office apps
  hand synuia ui                      # UI Automation: any native window
  cdp 127.0.0.1:9222 web              # a browser or any Electron app
  hands                               # all three, in routing order
  page web testbed #report            # register a page as a handle
  live web:testbed:#report            # -> hand=cdp-127.0.0.1:9222
  read web:testbed:#report h1
```

Verified live against Chrome and Edge at once, two hands in one session:
read, write, format, `struct` and `export` all land in the open page, `undo`
is refused rather than faked (a page has no undo this hand can honour), and
the allowlist and kill switch stop a live browser op exactly as they stop a
live Excel one. Reproduce it with `scripts/live-cdp-smoke.ps1`, which reaps
the browser tree and proves the port closed when it is done.

Start a Chromium for it with `--remote-debugging-port=9222` and its own
`--user-data-dir`. That port is unauthenticated: anything local that reaches
it controls the browser and its logged-in sessions, so keep it on 127.0.0.1
and off your everyday profile.

## Apps with no API at all

The UIA hand is the answer to "what about everything else". UI Automation
exposes a tree of elements with names, roles and values, so a model reads
**structure**, not pixels, and presses a control by identity rather than by
coordinate — a window that moves does not break it, and looking at the
screen costs no image tokens.

```
read   ui:Calculator::self :tree           # the map: Button id=num7Button name=Seven ...
invoke ui:Calculator::self id=num7Button   # press it by identity
read   ui:Calculator::self id=CalculatorResults   -> "Display is 12"
```

Verified live: 7 + 5 = 12 then Square = 144 driven entirely through the six
ops, plus `write` into Notepad's editor via ValuePattern. A selector that
matches nothing is refused with the display unchanged, and `allow word`
stops a `ui` op like any other. `invoke` is a `struct` verb, so it queues,
passes every gate and lands in the event feed; against a document model it
fails loudly rather than reporting a press that never happened.

Selectors are a closed `k=v` language (`name`, `id`, `type`, `class`, joined
by `,`). `name` falls back to a contains match because real names carry
punctuation and shortcut hints; `id` never does, since a fuzzy id match
presses the neighbouring button.

Reproduce with `scripts/live-uia-smoke.ps1`.

## The app

```
powershell -File scripts\console.ps1 -WithUia
```

Starts the app (and the UIA sidecar), prints a tokenised URL and opens it.
A thread list down the side, a conversation in the middle, a composer at the
bottom. Tool calls render as quiet one-line rows you can expand, so a run
that makes a dozen of them still reads as a conversation rather than a log.
An approval attaches to the composer with Approve / Deny, where your
attention already is, instead of scrolling past as a message.

Conversations are saved to `.syn/chats` after every turn and listed newest
first. Switching the model slot applies to the next turn of the conversation
you are in, which is what makes a 429 on a free-tier model survivable.

It binds 127.0.0.1 and every request carries a per-run token in a custom
header. A custom header forces a CORS preflight, which the server refuses,
so a page you happen to visit cannot post commands to this port — and a
request that arrives with any `Origin` header is rejected outright.

Stop it with `Get-Process ui, uia-host | Stop-Process`.

## Drive it from a terminal

```
cli.exe
  attach excel plan.xlsx Sheet1
  shellallow hostname            # empty by default: nothing may run
  task code                      # which router slot to think with
  do Read the sheet and write its shape into the report.
```

The loop takes one tool call per step, dispatches it through the same
`Runner` as a hand-typed op — kill switch, app allowlist, doom-loop gate,
registry check, event feed — and hands the result back fenced as untrusted
data. A `shell` call stops for a human:

```
CONFIRM hostname
        reason given: To get the machine name
        respond with `approve` or `deny <reason>`
```

A program that is not on the allowlist is refused *before* the prompt, so a
human is never asked to approve something that could not run. Denials are
reported to the model so it stops asking rather than looping.
