# Dev tools

Checks you run by hand, against the binaries you just built, for the kinds
of failure a unit test cannot see: what a real process does when it is
killed, how long a real turn takes against a real stream, what a real
command line exposes. Standard-library Python, like everything else here.
Each prints PASS or FAIL per check and exits 1 on any failure.

```
cd core && cargo build --bins && cd ..
python3 tests/dev/turn.py         # ~30s
python3 tests/dev/leftovers.py    # ~40s; ~60s more with LibreOffice
python tests/dev/window_close.py  # Windows, with a desktop: opens the real window for a few seconds
```

They are named so `python3 -m unittest discover -s tests` never picks them
up: they start processes, kill them, and take their time.

| tool | what it answers | caught, on the code before it existed |
|---|---|---|
| `turn.py` | does a turn end when the answer does, even if the host keeps the connection open; does the API key stay off every command line and leave no file; does a refused key say where to fix it | a turn waited 17s for a host holding the stream 8s; the key readable in curl's command line; "Check AGENT_API_KEY" |
| `leftovers.py` | when the CLI, the console or the MCP server is killed mid-work, is anything left running | curl left behind a killed CLI; the CLI and its curl left behind a killed console |
| `window_close.py` | when the real window is closed, does Syn end at once with nothing left running, does the console let go of its port, and does a sign-in the browser held a moment before survive the restart | closing the window ended Syn on the spot, the job object killed the browser before it wrote its cookies, and a site signed in to just before was signed out again |
| `fakemodel.py` | a fake OpenAI-compatible model the other two use; run it alone to point a console at | |

**Against another build.** `SYN_BIN_DIR=<dir>` points the tools at a
different `target/<profile>` -- which is how the right-hand column was
filled in: build an old commit with `CARGO_TARGET_DIR=<dir>` and run the
tool at it. A tool that passes against the code it was written to catch
checks nothing.

**The fake model.** `python3 tests/dev/fakemodel.py --port 8099` serves a
short streamed answer at `http://127.0.0.1:8099`. `--delay` is the time
between words, `--think` the silence before the first (a model reasoning
without streaming it), `--hold` how long the connection stays open after
`[DONE]`, and `--status 401` refuses every request. Set `AGENT_BASE_URL` to
it and any `AGENT_API_KEY` to watch the console against it.

**Platforms.** Written for and run on Linux. The process listing has a
Windows branch (PowerShell's `Get-CimInstance`) that `leftovers.py` and `window_close.py` have been run on;
the MCP scenario in `leftovers.py` needs LibreOffice and skips on Windows,
where the helper drives Office itself -- check that one by hand there:
start `mcpgate`, open a workbook, end `mcpgate` in Task Manager, and see
that `office-host.exe` goes too.

The console's own audit is `npm run audit` in `tests/ui`; see its README.
