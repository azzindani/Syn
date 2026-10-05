# Configuration

Syn reads its settings from environment variables. For convenience they can
live in a `.env` file: copy `.env.example` to `.env` and fill it in. `.env`
is gitignored.

- **Where `.env` is found:** `AGENT_ENV_FILE` if set; otherwise the nearest
  `.env` walking up from the working directory; otherwise one beside the
  executable. So `mcpgate.exe` finds the repository's `.env` wherever an MCP
  client starts it.
- **Precedence:** a variable already set in the environment always wins over
  the file, so `set AGENT_API_KEY=...` overrides `.env` for one session.
- **Format:** `KEY=value`, one per line; `#` comments, a leading `export`
  and one pair of surrounding quotes are accepted.

Secrets are read only when a request is sent. They are never logged, written
to a journal or a chat, or put in a prompt.

## API keys

A key can come from two places, and the first one found is used:

1. **Settings in the console** (or the CLI's `key <provider> <key>`). This
   is the way for an installed copy with no `.env`. The key is written to
   `auth.json` in `AGENT_HOME` (`.agent/auth.json` by default). On Windows
   it is sealed with DPAPI first, so the file holds `dpapi:...` and only
   the same Windows account on the same machine can read it back; copied to
   another account or machine it is useless and the key has to be added
   again. On Linux and macOS the file is plain text, readable by its owner
   only (mode 0600).
2. **The environment or `.env`**: `AGENT_API_KEY` and the rows below.

Removing a saved key in Settings goes back to the `.env` one, if there is
one. A key with a space or line break in it (a paste that caught two
things) is refused rather than trimmed.

## Model provider

| Variable | Default | Meaning |
|---|---|---|
| `AGENT_API_KEY` | — | Key for the provider at `AGENT_BASE_URL`. Needed for anything that calls a model (`do`, `say`, `send`, the console's chat), unless a key is saved in Settings. |
| `AGENT_BASE_URL` | `https://openrouter.ai/api/v1` | Any OpenAI-compatible chat-completions endpoint: OpenRouter, Groq, DeepInfra, Together, OpenAI, or a local llama.cpp / Ollama / LM Studio. |
| `AGENT_MODEL_SMALL` | `openrouter/auto` | Model for the `small` slot (the `skim` task). |
| `AGENT_MODEL_STANDARD` | `openrouter/auto` | Model for the `standard` slot (`routine`), the default. |
| `AGENT_MODEL_CODING` | `openrouter/auto` | Model for the `coding` slot (`code`). |
| `AGENT_MODEL_REASONING` | `openrouter/auto` | Model for the `reasoning` slot (`deep`, and the vision fallback). |
| `AGENT_BASE_URL_<SLOT>`, `AGENT_API_KEY_<SLOT>` | the global pair | Put one slot on a different provider (`SMALL`, `STANDARD`, `CODING`, `REASONING`). |
| `AGENT_API_KEY_OPENCODE` | — | The key for OpenCode Zen. Its models are in the console's picker either way; this is what lets a turn be sent to one. |
| `AGENT_API_KEY_OPENCODE_GO` | — | The key for OpenCode Go (the subscription, `https://opencode.ai/zen/go/v1`), likewise. A Zen key does not work here. Go asks each client to name itself and its conversation, so Syn sends `User-Agent: syn/<version>` and the chat's id as `x-opencode-session` to OpenCode (the id only there). |
| `AGENT_API_KEY_OPENROUTER` | — | The key for OpenRouter when `AGENT_BASE_URL` points elsewhere, likewise. |
| `AGENT_AUTH_CONTENT` | — | The whole credentials file (`auth.json`) as JSON, for machines with nowhere to keep a file. While it is set, Settings cannot save a key, because a saved one would never be read. |

When a model fails with a rate limit or an outage, Syn waits and retries the
same model once, then walks the other slots. Slots that resolve to the same
model are skipped, so giving the slots different models (and ideally
different providers) is what makes the fallback useful.

## Replies

| Variable | Default | Meaning |
|---|---|---|
| `AGENT_STREAM` | on | `0` waits for whole replies (up to five minutes each) instead of streaming, for a gateway that mishandles `"stream": true`. |
| `AGENT_STREAM_IDLE_SECS` | `180` | How long a streamed reply may send nothing at all before it is abandoned (10–3600). Providers send keep-alives while a model thinks, so this detects a dead connection, not a slow model. |
| `AGENT_OFFICE_CALL_SECS` | `120` | Seconds an Office call may run before the Office helper presses Esc at Excel to stop a calculation, undoes what the call changed and tells the model why (5–86400). Read by `office-host`. See `docs/troubleshooting.md`, item 12. |
| `AGENT_OFFICE_LONG_SECS` | `1200` | The same for calls that are slow by nature (open, export, save, close): no Esc, only an answer to the caller after this long and a grace period. |
| `AGENT_OFFICE_IDLE_SECS` | `60` | How long the keyboard and mouse must have been untouched before the Office helper presses Esc to leave a cell that Excel is stuck editing (0 = always). With anyone at the keyboard it only says so. See `docs/troubleshooting.md`, item 15. |
| `AGENT_MAX_STEPS` | `100` | Tool calls the agent loop may make in one turn. |
| `AGENT_CONTEXT_CHARS` | `240000`, less for small models | How much conversation the loop keeps before compacting. It shrinks automatically to fit the context window the provider reports for the chosen model. |
| `AGENT_PLAN` | on | `0` removes the `plan` tool from the loop. |
| `AGENT_MANUAL` | on | `0` removes the `manual` tool from the loop. |

## Applications

No application is connected unless it is named here: a pipe name that is
wrong would connect to something else, so there is no built-in default.

| Variable | Example | Meaning |
|---|---|---|
| `AGENT_PIPE_EXCEL`, `AGENT_PIPE_WORD`, `AGENT_PIPE_PPT` | `hand-excel`, `hand-word`, `hand-powerpoint` | The named pipe (Unix socket off Windows) each Office helper listens on. `off` stops offering that application. |
| `AGENT_PIPE_UIA` | none: not offered | The UI Automation helper, for any native Windows window. `.env.example` sets `hand-uia`. |
| `AGENT_BROWSER` | Chrome, then Edge | The browser Syn starts when a page is first wanted: a program's path, or `off` to give the model no browser at all. Unset, Syn looks in the usual places for Chrome and then Edge (every Windows machine has Edge). It runs on a profile of its own in `.agent\browser-profile`, apart from yours, so a site signed in to once stays signed in; delete that folder to sign out of everything. It listens on loopback on a port the system picks, and stops when Syn does. |
| `AGENT_BROWSER_HEADLESS` | off | `1` runs that browser with no window, for a server or a test. |
| `AGENT_CDP` | none | Drive a browser or Electron app that is **already running** instead: one started with `--remote-debugging-port` and a `--user-data-dir` of its own. Syn attaches and never starts or closes it, and `AGENT_BROWSER` is not used. That port is unauthenticated: keep it on 127.0.0.1. Chrome ignores the port on your everyday profile. |
| `AGENT_OFFICE_HOST` | — | Path to `office-host.exe` (or `sidecar-lo/lo_host.py` off Windows), when not in this repository's Release build output. |
| `AGENT_UIA_HOST` | — | Path to `uia-host.exe`, likewise. |
| `AGENT_PYTHON` | `python3` | The Python that has LibreOffice's `uno` module, for `lo_host.py`. |
| `AGENT_LO_VISIBLE` | off | `1` shows the LibreOffice windows instead of running headless (needs a display). |
| `AGENT_TRACE` | off | `1` makes the helpers write a trace of every call to stderr. |
| `AGENT_LAUNCH` | on | The console and CLI: `0` never starts a helper, and only attaches ones already running. (MCP has its own, `AGENT_MCP_LAUNCH`.) |

## MCP server

| Variable | Default | Meaning |
|---|---|---|
| `AGENT_MCP_APPS` | all | Only these apps may be opened, e.g. `excel,word`. Checked before anything is launched; a name that is not an app allows nothing. |
| `AGENT_MCP_ROOTS` | anywhere | `open` only accepts files under these folders, and `export` only writes there; separated by `;`. |
| `AGENT_MCP_LAUNCH` | on | `0`: never start a helper, only connect to ones already running. |

## Safety switches

| Variable | Default | Meaning |
|---|---|---|
| `AGENT_VBA` | off | `1` allows `struct` `macro` (writing and running VBA) for processes started with it. See [security.md](security.md). |

The `shell` allowlist is not an environment variable: it is empty at start,
and programs are added for a session with the CLI's `shellallow` command.

## Storage

| Variable | Default | Meaning |
|---|---|---|
| `AGENT_HOME` | `.agent` beside the `.env` in use | Where Syn keeps its own state: `chats/` (conversations, one JSON Lines file each), `live/` (progress logs the console follows) and `models.json` (the cached model list). Gitignored. |

Keys saved in Settings live in `auth.json` in the same folder (or come from
`AGENT_AUTH_CONTENT`); see [API keys](#api-keys).
