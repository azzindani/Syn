# Development

## Build and test

```
cd core
cargo clippy --all-targets -- -D warnings   # CI fails on any warning
cargo test                                   # offline: no key, no Office, no network
cd ..
python3 -m unittest discover -s tests        # the relay prototype, and LibreOffice live tests if available
```

The console UI tests need a fresh build, because the page is compiled into
`ui`:

```
cd core && cargo build --bins
cd ../tests/ui && npm install && npm test
```

See `tests/ui/README.md` for the specs, and `node showcase.mjs` for
screenshots of every console state in light and dark, desktop and phone.

CI (`.github/workflows/ci.yml`) runs clippy and the Rust tests on Linux,
macOS and Windows; builds both C# helpers; parses every PowerShell script;
runs the prototype's tests; and runs the LibreOffice live tests with
LibreOffice installed.

## Test tiers

What can be proven depends on what the machine has:

| Tier | Needs | Proves |
|---|---|---|
| 1 | nothing | The loop, the parser, the schemas, the gates, and that the Rust and C# sides of the wire agree. `cargo test`, on any OS. |
| 2 | an API key | How a model behaves over a long run, against Syn's in-memory documents (`attach` without `live`). No formulas and no Office-only verbs. |
| 2½ | LibreOffice + `python3-uno` | The whole stack live — MCP, `open`, the gates, the wire, the guidance — against a real office engine, on real `.xlsx`/`.docx`/`.pptx`, formulas included. `tests/test_lo_live.py`. |
| 3 | Windows + Microsoft Office | That the COM calls are right and the documents end up correct. `scripts\live-office-peak.ps1`. |

Tier 2½ runs the real `mcpgate` against `sidecar-lo/lo_host.py`:

```
sudo apt-get install libreoffice-calc libreoffice-writer libreoffice-impress python3-uno
cd core && cargo build --bins && cd ..
/usr/bin/python3 -m unittest discover -s tests -p test_lo_live.py -v
```

It proves everything above the C# helper. It cannot prove the C#: a verb
working on LibreOffice says nothing about the COM call behind it. Anything
touching `sidecar-csharp` is unverified until tier 3 has run it; say so in
the commit or pull request ("compiles in CI, tier 1 green, needs a live check
on Word").

## Contracts that keep the sides in step

These tests read source files as text and fail when they disagree:

| Test | Holds |
|---|---|
| `core/tests/wire_contract.rs` | Every method Rust can send has a handler in the C# helpers; every `struct` verb reaches the wire; each app's verb table (`tools::APP_METHODS`) matches its dispatcher both ways; the LibreOffice helper handles only methods Rust sends. |
| `core/tests/coach_contract.rs` | Every error-explaining rule still matches text a helper really sends. |
| `core/tests/console_contract.rs` | Every progress line the CLI prints has a place in the console, and the console draws nothing the CLI never sends. |
| `core/tests/mcp_stdio.rs` | The real `mcpgate` binary over stdio. |
| `core/tests/cli_turns.rs` | The real `cli` binary against a local fake provider, streaming included. |

## Adding a capability

Almost every new capability is a new **`struct` verb**, not a new operation.

**A plain verb** (named fields sent to the application, such as `sort` or
`textBox`):

1. A row in `tools::OFFICE_VERBS` in `core/src/tools.rs`: its required and
   optional fields, and which one is the long payload. Parsing and the wire
   envelope follow from the row.
2. The verb in the `struct` schema's enum and in `STRUCT_VERBS`, and in the
   per-app lists: `tools::APP_METHODS` and the schema description (a test
   checks the two agree).
3. A case in the right app's method switch in
   `sidecar-csharp/Host/Program.cs`, with the implementation in `Verbs.cs`.
   Late-bound COM; remember that PowerPoint's `Visible`, `DisplayAlerts`,
   `HasTextFrame` and `HasTable` are `MsoTriState` (`-1` for true), not
   `bool`. If the verb changes a document, check that `Undo.cs` covers what
   it changes.
4. The same verb in `sidecar-lo/lo_host.py` if UNO does it plainly;
   otherwise it is refused there by name, never faked.
5. A line in `core/src/manual.rs` if the verb has a non-obvious idiom.
   Reference only: the manual must not name a test fixture or prescribe an
   order of steps (a test enforces this).
6. Words for the console in `core/src/labels.rs`.
7. A step in `scripts/live-office-peak.ps1`, which is how the verb is proven
   on Windows.
8. Tests at tier 1 (schema, envelope, refusal) and, where LibreOffice can do
   it, in `tests/test_lo_live.py`.

**A verb that needs its own parsing** adds a variant to `StructArgs` in
`core/src/ops.rs`, a case in `hand::envelope_for`, and optionally a CLI
command in `core/src/bin/cli.rs`, on top of the steps above.

## Rules that are load-bearing

- **Zero dependencies in `core`.** The WebSocket client, HTTP server, JSON
  and SSE handling are written with `std`. Write the small thing and test it;
  do not add a crate.
- **One path to a document.** Every caller uses `Runner::run`. A new entry
  point that calls `ops::execute` or a hand directly skips every gate.
- **Closed schemas; refuse, don't truncate; results are untrusted.** See
  [security.md](security.md).
- **Guidance for small models.** Tool results say what to do next as a call
  that can be copied exactly; refusals say how to fix the call; ambiguous
  input is refused with the alternatives. MCP `instructions` stay short (a
  test caps them).
- **The helpers never close or save what they did not open,** and office-host
  makes every COM call from its one STA thread.
- **Vendor-neutral naming.** Model slots are named by job (`small`,
  `standard`, `coding`, `reasoning`); no vendor model names in code, config,
  docs or tests.
- **Capability-test honesty.** `tests/capability/` holds recorded runs.
  Never re-score a recorded run or change a check after seeing one;
  pre-register an experiment in `tests/capability/SPEC-v3.md` before running
  it.

## Conventions

- Comments explain *why*, often with the incident that taught the lesson.
- Test names are sentences that state the behaviour
  (`a_dead_pipe_drops_only_its_own_hand`).
- Commit subjects are plain-English sentences about the effect; the body says
  what broke, how it was found, and what remains unverified.
- Keep `README.md`, `docs/` and `.env.example` in step with the code: a
  setting the docs name that the code does not read is a bug.
