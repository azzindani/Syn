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

## Model provider

| Variable | Default | Meaning |
|---|---|---|
| `AGENT_API_KEY` | — | Key for the provider at `AGENT_BASE_URL`. Needed for anything that calls a model (`do`, `say`, `send`, the console's chat). |
| `AGENT_BASE_URL` | `https://openrouter.ai/api/v1` | Any OpenAI-compatible chat-completions endpoint: OpenRouter, Groq, DeepInfra, Together, OpenAI, or a local llama.cpp / Ollama / LM Studio. |
| `AGENT_MODEL_SMALL` | `openrouter/auto` | Model for the `small` slot (the `skim` task). |
| `AGENT_MODEL_STANDARD` | `openrouter/auto` | Model for the `standard` slot (`routine`), the default. |
| `AGENT_MODEL_CODING` | `openrouter/auto` | Model for the `coding` slot (`code`). |
| `AGENT_MODEL_REASONING` | `openrouter/auto` | Model for the `reasoning` slot (`deep`, and the vision fallback). |
| `AGENT_BASE_URL_<SLOT>`, `AGENT_API_KEY_<SLOT>` | the global pair | Put one slot on a different provider (`SMALL`, `STANDARD`, `CODING`, `REASONING`). |
| `AGENT_API_KEY_OPENCODE` | — | Adds OpenCode Zen's models to the console's model picker. |
| `AGENT_API_KEY_OPENROUTER` | — | Adds OpenRouter's models to the picker when `AGENT_BASE_URL` points elsewhere. |
| `AGENT_AUTH_CONTENT` | — | The whole credentials file (`.agent/auth.json`) as JSON, for machines with nowhere to keep a file. |

When a model fails with a rate limit or an outage, Syn waits and retries the
same model once, then walks the other slots. Slots that resolve to the same
model are skipped, so giving the slots different models (and ideally
different providers) is what makes the fallback useful.

## Replies

| Variable | Default | Meaning |
|---|---|---|
| `AGENT_STREAM` | on | `0` waits for whole replies (up to five minutes each) instead of streaming, for a gateway that mishandles `"stream": true`. |
| `AGENT_STREAM_IDLE_SECS` | `180` | How long a streamed reply may send nothing at all before it is abandoned (10–3600). Providers send keep-alives while a model thinks, so this detects a dead connection, not a slow model. |
| `AGENT_MAX_STEPS` | `40` | Tool calls the agent loop may make in one turn. |
| `AGENT_CONTEXT_CHARS` | `240000`, less for small models | How much conversation the loop keeps before compacting. It shrinks automatically to fit the context window the provider reports for the chosen model. |
| `AGENT_PLAN` | on | `0` removes the `plan` tool from the loop. |
| `AGENT_MANUAL` | on | `0` removes the `manual` tool from the loop. |

## Applications

No application is connected unless it is named here: a pipe name that is
wrong would connect to something else, so there is no built-in default.

| Variable | Example | Meaning |
|---|---|---|
| `AGENT_PIPE_EXCEL`, `AGENT_PIPE_WORD`, `AGENT_PIPE_PPT` | `hand-excel` | The named pipe (Unix socket off Windows) each Office helper listens on. |
| `AGENT_PIPE_UIA` | `hand-uia` | The UI Automation helper, for any native Windows window. |
| `AGENT_CDP` | `127.0.0.1:9222` | A Chromium browser or Electron app started with `--remote-debugging-port`. That port is unauthenticated: keep it on 127.0.0.1 and use a separate `--user-data-dir`. |
| `AGENT_OFFICE_HOST` | — | Path to `office-host.exe` (or `sidecar-lo/lo_host.py` off Windows), when not in this repository's Release build output. |
| `AGENT_UIA_HOST` | — | Path to `uia-host.exe`, likewise. |
| `AGENT_PYTHON` | `python3` | The Python that has LibreOffice's `uno` module, for `lo_host.py`. |
| `AGENT_LO_VISIBLE` | off | `1` shows the LibreOffice windows instead of running headless (needs a display). |
| `AGENT_TRACE` | off | `1` makes the helpers write a trace of every call to stderr. |

## MCP server

| Variable | Default | Meaning |
|---|---|---|
| `AGENT_MCP_APPS` | all | Only these apps may be opened, e.g. `excel,word`. Checked before anything is launched; a name that is not an app allows nothing. |
| `AGENT_MCP_ROOTS` | anywhere | `open` only accepts files under these folders, separated by `;`. |
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

Stored credentials, if any, are read from `.agent/auth.json` in the working
directory (or from `AGENT_AUTH_CONTENT`).
