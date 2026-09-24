# Desktop Agent — one brain, many hands

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
  - `mcpgate` — MCP server: the six document ops generated from `tools`,
    plus `status`, `open` and `manual`, for any MCP client
    (`docs/11-mcp.md`). Every call goes through `Runner`.
  - `desk` — how a session gets a document open: connect a hand, start its
    helper if it is not running, open the file, bind the handle live.
  - `json` — a small JSON parser, for the one component that speaks a
    nested protocol.
  - `catalog` — the model list: each provider's own `GET /models`,
    cached in `.agent/models.json` and refreshed while the console runs.
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
    `mcpgate` (MCP server on stdio), `ui` (the console).
- `widget/index.html` — the chat app, served by the `ui` binary: thread list,
  conversation, composer. It posts one command line to a loopback server
  that types it into a real `cli` process, so the page can only do what the
  CLI can do. Layout digested from t3code (`docs/DIGEST-05-t3code-ui.md`).
  (Tauri shell in `widget/src-tauri`, still unbuilt.)
- `core/src/chats.rs` — saved conversations as JSON Lines under
  `.agent/chats` (gitignored). One file per thread, appended per turn.
- `sidecar-csharp/Host/` — full STA COM sidecar (Word/Excel/PowerPoint,
  named-pipe office-rpc/1, timeout-guarded calls, .bak snapshots).
  Needs Windows + .NET 8 + Office to compile/run.
- `sidecar-csharp/Uia/` — the UI Automation sidecar (`uia-host`): reads any
  native window's control tree and presses controls by identity. Same
  office-rpc/1 wire, so core reaches it through the same `Hand`. MTA, not
  STA — a UIA client must not be STA or it can deadlock against an STA
  provider, which is the opposite of what the Office sidecar needs.
- `sidecar-lo/` — the helper for machines without Microsoft Office: the same
  `office-rpc/1` as the COM sidecar, driving LibreOffice (Calc, Writer,
  Impress) over a Unix socket. It lets the whole stack run live on Linux and
  in CI; it does not test the C#. See `sidecar-lo/README.md`.
- `office-pane/` — Office.js task pane (Mac/Web hand), sideload to verify.
- `scripts/` — Windows bring-up: `new-testbed-docs.ps1` (fixtures via COM),
  `live-excel-smoke.ps1` (M1/M2 acceptance vs real Excel), `pipe-client.ps1`,
  `live-cdp-smoke.ps1` (a real browser), `live-uia-smoke.ps1` (Calculator).
- `testbed/` — gitignored run area for live Office tests (docs/out/logs).
- `.env.example` — provider key, endpoint, and the four model slots
  (`AGENT_MODEL_SMALL|STANDARD|CODING|REASONING`); copy to `.env`, which is
  gitignored. Unset slots default to `openrouter/auto`.
- `tests/capability/` — the v0.1.0 capability test: brief, rubric, scorers,
  and every recorded run (`SPEC-v3.md`).
- `tests/ui/` — Playwright specs that drive the real console.
- `docs/` — 00→11, DIGEST-00→09, PRD, ideas, runbook-windows.
- `.tmp/repos/` — gitignored; local clones of the sources this was ported
  from (`docs/DIGEST-00-inventory.md`).

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
python3 -m unittest discover -s tests      # relay POC
cd core && cargo clippy --all-targets -- -D warnings && cargo test
cargo build --release && cargo run --example demo
./target/release/cli < ../tests/e2e_script.txt
./target/release/cli < ../tests/e2e_safety.txt   # allow/kill/journal/replay/sessions
# live, against LibreOffice (needs libreoffice-calc/-writer/-impress + python3-uno):
cd .. && /usr/bin/python3 -m unittest discover -s tests -p test_lo_live.py -v
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | cargo run -q --bin mcpgate
```
## Run on Windows (live Office)
```
dotnet build sidecar-csharp\Host\Host.csproj -c Release
powershell -File scripts\new-testbed-docs.ps1     # fixtures (close Office first)
powershell -File scripts\live-excel-smoke.ps1     # attach + read/write live Excel
```
Verified on Windows 11 + Microsoft 365 + .NET 8: the sidecar attaches to an
open workbook, reads and writes it live, fails closed on bad handles, and
detaches without closing the user's Excel — driven both by the PowerShell
smoke test and by `cli.exe` itself over the pipe (`hand` / `lread` / `lwrite`).
Word and PowerPoint were driven live by the capability test's scripted
control (`tests/capability/load.txt`: 217 operations across all three apps).
The Tauri bundle is still unbuilt — status and the COM lessons behind the
fixes are in `docs/runbook-windows.md`.

Copy `.env.example` to `.env` and add a key to enable `send` and `do`.

## Many hands, one session

`Runner` holds several hands at once and routes each handle to the one that
claims its app, so the caller names a document and never a transport. A
named claim beats a catch-all; one dead transport drops only its own hand.

```
cli.exe
  hand hand-excel excel word ppt   # COM sidecar, Office apps
  hand hand-uia ui                      # UI Automation: any native window
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

Starts the app (and the UIA sidecar), prints its address and opens it.
A thread list down the side, a conversation in the middle, a composer at the
bottom. Each turn's tool calls sit in one card headed by a sentence —
"Working in Word" with a running clock while it runs, "Worked in Excel and
PowerPoint · 1 refused · 6 steps" when it is done — and every row inside
carries the colour of the app it touched, so a run that makes a dozen calls
still reads as a conversation rather than a log. An approval attaches to the
composer with Approve / Deny, where your attention already is, instead of
scrolling past as a message. The status menu top right connects apps,
switches between system, light and dark, and sets contrast. The visual
design is digested from t3code in `docs/DIGEST-09-t3code-visual.md`.

Conversations are saved to `.agent/chats` after every turn and listed newest
first.

The model button in the composer opens a searchable list of every model
your provider serves that can call tools, with its context size, its price
per million tokens and whether it can think. The list is the provider's own
`/models`, fetched at start, again every 15 minutes while the console runs,
and on demand with the refresh button, so a model released today shows up
without editing anything. It lists OpenRouter by default, and also OpenCode
Zen once `AGENT_API_KEY_OPENCODE` is set; an endpoint of your own in
`AGENT_BASE_URL` gets its list read the same way. "Automatic" means the model
in `.env`. Beside it, the thinking level (auto, low, medium, high) sets how
hard the model reasons before it answers; auto leaves it to Syn. Both apply
to the next turn of the conversation you are in, which is what makes a 429
on a free-tier model survivable: pick another and send again.

It binds 127.0.0.1 only, and a command is refused unless its `Origin` is
the console's own (and refused outright with none). Browsers attach
`Origin` to every cross-origin POST and cannot forge it, so a page you
happen to visit cannot post commands to this port; preflights are refused
too. See the header of `core/src/bin/ui.rs`.

Stop it with `Get-Process ui, uia-host | Stop-Process`.

## Drive it from any MCP client

```
cargo build --release          # core\target\release\mcpgate.exe
```

Point Claude Desktop, Claude Code, OpenCode or any MCP client at
`mcpgate.exe`. The model gets `status`, `open`, the six document ops and
`manual`. `open` starts Excel, Word or PowerPoint, and the helper that
connects to it, when they are not running, then hands back the handle to
use. Every call passes the same gates as Syn's own loop, results from
documents come back fenced as untrusted, and each call appears live in
Syn's console. The guidance is written so that small and mid-size models
can follow it, not only frontier ones. Setup, settings and what is still
unverified on real Office: `docs/11-mcp.md`.

## Drive it from a terminal

```
cli.exe
  attach excel plan.xlsx Sheet1
  shellallow hostname            # empty by default: nothing may run
  models atlas                   # search the providers' model lists
  model openrouter vendor/model  # talk to that model (model auto: the .env slot)
  think high                     # low|medium|high, or auto
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
