# CLAUDE.md

Syn is a desktop agent for one user on one machine. One chat session and one
event feed drive the apps already open: Word, Excel and PowerPoint through
COM, native windows through UI Automation, and browsers or Electron apps
through Chrome DevTools. Any OpenAI-compatible chat-completions endpoint
works (OpenRouter by default). Start with `README.md`. The design history is
in `docs/`, and `docs/09` and `docs/10` are the most current.

## Layout

- `core/`: the Rust core (edition 2024) with **zero dependencies**, and it
  stays that way. The WebSocket client, HTTP server, JSON handling and SSE
  parsing are all hand-written with std, and the model is reached through
  `curl`. Don't add a crate: write the small thing, and test it.
  - `agent.rs`: the loop. The model sits behind the `Brain` trait, so the
    whole loop is tested offline with scripted replies.
  - `runner.rs`: the **only** path to a document. Kill switch, app
    allowlist, repeated-call gate, registry check, event feed. Never add a
    second path around it.
  - `tools.rs`: the tools a model can call, which are the six ops (`read`,
    `write`, `format`, `struct`, `export`, `undo`) plus `shell`.
    `looptools.rs` holds tools the loop answers itself (`manual`, `plan`),
    and `surface.rs` merges the two at the wire.
  - `hand.rs`: the `office-rpc/1` transport (JSON lines over a named pipe).
    `cdp.rs` + `ws.rs` are the browser hand.
  - `provider.rs`, `config.rs`, `router.rs`: request bodies, `.env`
    loading, and the four model slots.
  - `mcpgate.rs` + `desk.rs`: the MCP server (`docs/11-mcp.md`). Its
    document tools are generated from `tools::TOOLS` and a test fails if
    they drift; `desk` connects hands, starts helpers and opens files.
    Every MCP call goes through `Runner::run`, like the loop's.
  - `json.rs`: a real JSON parser. Use it for anything nested; the
    substring `tools::field` finds the first matching key at any depth.
  - bins: `cli` (the REPL), `ui` (a loopback web console that drives `cli`
    over stdin/stdout, serving `widget/index.html`), `mcpgate` (the MCP
    server on stdio; stdout carries protocol only, notes go to stderr).
- `sidecar-csharp/Host`: the Office COM sidecar (STA). `sidecar-csharp/Uia`:
  the UI Automation sidecar (MTA). Both need Windows and .NET 8 to build,
  and Office to run.
- `sidecar-lo/lo_host.py`: the same `office-rpc/1`, driving LibreOffice over
  a Unix socket. Off Windows, `mcpgate` and the REPL start it instead of
  office-host. Keep its replies worded like office-host's; a verb it does
  not implement is refused, never faked.
- `relay/`: the original Python proof of concept, stdlib only.
- `protocol/`: the rpc catalog and the default-deny security policy (JSON).
- `widget/index.html`: the whole console UI, one file, no dependencies,
  compiled into `ui`. Its visual design is digested from t3code in
  `docs/DIGEST-09-t3code-visual.md`. It keys icons off a row's `app` and
  `tool`, never off a verb (`core/tests/console_contract.rs`).
- `tests/ui/`: Playwright against the real console.
  `tests/capability/`: the v0.1.0 capability test and its recorded runs.
- `scripts/`: Windows bring-up and live smoke tests (PowerShell).

## Commands

```
cd core
cargo clippy --all-targets -- -D warnings   # CI fails on any warning
cargo test                                   # offline: no key, no Office, no network
python3 -m unittest discover -s tests        # relay POC, from the repo root
```

CI (`.github/workflows/ci.yml`) runs clippy and the tests on Linux, macOS and
Windows. It also builds both C# sidecars and checks that every `.ps1`
parses. Run clippy and the tests before every commit.

The UI specs need a fresh build, because the page is compiled into `ui`:
`cd core && cargo build --bins`, then `cd tests/ui && npm install && npm
test`. In the cloud sandbox don't run `playwright install`: use the
preinstalled Chromium with `PW_CHROMIUM=/opt/pw-browsers/chromium npm test`,
and copy `.env.example` to `.env` first (one spec needs a wired hand). After
a UI change, `node showcase.mjs` writes screenshots of every state, dark and
light, desktop and phone, to `testbed/shots/showcase`: look at them.

## What a cloud session can and cannot verify

There are three tiers (`docs/10-adding-tools-remotely.md`):
1. `cargo test`, anywhere. This covers the loop, the parser, the schemas,
   and `core/tests/wire_contract.rs`, which fails if a method `hand.rs` can
   send has no handler in the C# sidecars.
2. An API key plus an in-memory document (`attach` without `live`). This
   shows how a model behaves over a long run, but has no formulas and no
   Office-only verbs.
   2½. LibreOffice, live: `tests/test_lo_live.py` runs the real `mcpgate`
   against real files through `sidecar-lo` (this sandbox can install
   `libreoffice-calc libreoffice-writer libreoffice-impress`, and has
   `python3-uno`; run it with `/usr/bin/python3`). It proves everything
   above the C#, formulas included. Use it for any change to MCP, the desk,
   the wire or the guidance.
3. Windows with Office installed, the only way to prove a COM call is
   right.

From Linux, anything touching the sidecars is unverified live. Say so
plainly in commits and PRs, e.g. "compiles in CI, tier 1 green, needs a
live check on Word".

## Adding a `struct` verb

This is almost always the right shape for a new capability. Don't add a
seventh op. The steps are, in order: `ops.rs` (variant) → `tools.rs` (schema
enum + `STRUCT_VERBS`) → `hand.rs` (`envelope_for`) →
`sidecar-csharp/Host/Program.cs` (a case in the method switch) → `cli.rs`
(an optional command) → `manual.rs` (reference only: it must never name the
fixture or prescribe an order, and a test enforces this) → tests. The full
recipe is in `docs/10`.

## Rules that are load-bearing

- **Closed schemas.** Use `additionalProperties:false` and put a cap on
  every string. Oversized input is refused, never truncated. Unknown keys
  and verbs are rejected, never ignored.
- **Results are untrusted.** Anything read from a document or a program
  goes back to the model marked as untrusted data, never as instructions.
  That holds for MCP results too (`mcpgate::fenced`).
- **One road to a document.** Every caller (the loop, the REPL, MCP) uses
  `Runner::run`. A new entry point that calls `ops::execute` or a hand
  directly skips every gate; don't add one.
- **Write MCP guidance for a small model.** Tool results say what to do
  next as a call that can be copied exactly (a test runs it); refusals say
  how to fix the call; ambiguous input is refused with the alternatives,
  never guessed. Keep `INSTRUCTIONS` short (a test caps it).
- **`shell` always stops for a human.** Its allowlist is empty by default,
  and a program not on it is refused before anyone is asked. VBA (`struct`
  `macro`) is off unless `AGENT_VBA=1`, and that gate lives in
  `Runner::pump`.
- **Real control is COM.** On Windows every document op goes Rust →
  named pipe → C# helper → COM (or UI Automation) into the running app.
  Python appears only in `sidecar-lo` (LibreOffice, for testing off
  Windows) and the `relay/` prototype; never on the Windows control path,
  and never Python office-file libraries (openpyxl, python-docx, ...).
- **The sidecar never closes or saves what it did not open.** Office COM
  servers are single-instance per user, so quitting one can close the
  user's own work. Read the lessons in `docs/runbook-windows.md` §0 before
  editing the C#. In particular, PowerPoint's `Visible`, `DisplayAlerts`,
  `HasTextFrame` and `HasTable` are `MsoTriState`, not bool.
- **Capability-test honesty.** Never re-score a recorded run, and never
  add, remove or loosen a check after seeing a run. Pre-register an
  experiment in `tests/capability/SPEC-v3.md` before running it. Void
  results stay in the file, marked void, and are never deleted.

## Naming: stay neutral

The project is vendor-neutral. Model slots are named by job: `small`,
`standard`, `coding`, `reasoning`. The env vars are
`AGENT_MODEL_SMALL|STANDARD|CODING|REASONING`, with matching
`AGENT_BASE_URL_*` and `AGENT_API_KEY_*`. The CLI task names are `skim`,
`routine`, `code`, `deep` and `vision`. Every slot defaults to
`openrouter/auto`. Don't bring vendor model names or tier names into code,
config, docs or tests. The one exception is a verbatim quote of someone
else's source in a `docs/DIGEST-*` note.

## Style

- Comments explain *why*, often with the incident that taught the lesson.
  Match that density, and keep the reasons when you change the code.
- Test names are sentences that state the behaviour
  (`a_dead_pipe_drops_only_its_own_hand`).
- Commit subjects are plain-English sentences about the effect ("Stop
  dropping tool calls, and let a cell hold a comma"). The body says what
  broke, how it was found, and what remains unverified.
- Keep `README.md` and `.env.example` in step with the code. A config name
  in the docs that the code doesn't read is a bug.
