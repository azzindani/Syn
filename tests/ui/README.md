# UI tests

Playwright against the real console: it starts `ui` (`ui.exe` on Windows) on
port 7799 (not the 7777 you browse), drives the page, and stops it again.

```
cd core && cargo build --bins
cd ../tests/ui && npm install && npx playwright install chromium
npm test
```

On a machine that already has a Chromium Playwright can drive (a cloud
sandbox, say), skip the install and point at it instead:
`PW_CHROMIUM=/opt/pw-browsers/chromium npm test`.

`chat.spec.mjs` "the machinery sits behind the status control" needs at
least one hand wired, i.e. a `.env` with an `AGENT_PIPE_*` or `AGENT_CDP`
line; copying `.env.example` to `.env` is enough.

**Looking at it.** `node showcase.mjs` starts a console of its own, stages
the states a person actually sees — the first screen, a run in progress, an
approval, the status menu — and writes a PNG of each in dark and light, at
desktop and phone width, to `testbed/shots/showcase`. It asserts nothing;
it is for judging the design by eye, which no spec can do.

**Rebuild after editing the page.** `widget/index.html` is `include_str!`'d
into `ui.exe`, so a spec run against a stale binary tests the previous page
and passes. `global-setup.mjs` refuses to start when the page is newer than
the binary, because that failure is silent and completely convincing.

| spec | what it answers | needs | costs |
|---|---|---|---|
| `harness.spec.mjs` | does a command reach a document, change it, and stop when a gate says stop | nothing | ~9s |
| `render.spec.mjs` | is what a human reads legible | nothing | ~5s |
| `stream.spec.mjs` | is the live view actually live | nothing | ~26s |
| `chat.spec.mjs` | does a typed prompt reach a model and come back | `AGENT_API_KEY` and whichever model `.env` wires to the current slot | one model call |

Everything but `chat.spec.mjs` is offline: no key, no network, no Office.
`npx playwright test --grep-invert "reaches the model"` runs that set in
about 35 seconds.

**Specs that need their own console say so.** `kill` latches the Runner for
good — "fresh guard required" — and `allow` locks the allowlist, so those
run against a console started by `startConsole()` and stopped afterwards.
`render.spec.mjs` and the screenshot test also take one, pinned with
`--tail` to a log they own: otherwise any other console on the machine
becomes the newest run, the page follows it, and the timeline under test is
wiped. Sharing one console cost an afternoon of failures that looked like
bugs in unrelated tests.

**Do not wait for `networkidle`.** The page holds an event-stream open the
whole time it is on screen, so "no network for 500ms" is a state it never
reaches. Wait for `window.live.sse` or `window.live.source` instead.

`window.live` is the page's observation surface — `sse`, `streaming`,
`polling`, `source`, `rows`, and `step()`, `quiet()`, `toPolling()`. The
page's own state is script-scoped, and `let` does not put anything on
`window`; the first version of the stream specs read `window.sseOK`, got
`undefined`, and failed for that reason alone.

`render.spec.mjs` drives the rendering directly: it hands `liveStep` the
exact lines the CLI prints and checks what a human ends up reading — that a
tool call reads as a sentence and never as a tool name or a JSON blob, that
a refusal and a broken run do not look alike, that twelve calls stay a
screen of quiet rows, and that the prose lines (`STEP`, `REFUSED`, `DID`)
are ignored so rewording one cannot blank the view. It stops the page's own
pollers first, so it measures the rendering and not the timing.

`core/tests/console_contract.rs` is the cheap half of the same contract: it
reads `cli.rs` and `index.html` as text and fails if the CLI emits a receipt
the page cannot draw, or the page draws one nothing emits. No browser, no
Node, under a second, and it runs in CI on three operating systems.

Screenshots land in `testbed/shots/` (gitignored): `console-run.png`,
`console-run-expanded.png`, `console-run-light.png`.
