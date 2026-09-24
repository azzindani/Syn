# The console and the CLI

Syn's own agent can be driven two ways: the **console**, a local web app, and
the **CLI**, a terminal REPL. The console is a front end for the CLI: it types
each command into a real `cli` process and draws what the CLI prints, so it
can do exactly what the CLI can do and nothing more.

## The console

```
powershell -File scripts\console.ps1              # http://127.0.0.1:7777/
powershell -File scripts\console.ps1 -WithUia     # also start the UI Automation helper
powershell -File scripts\console.ps1 -Port 7788 -NoOpen
```

On Linux or macOS run `core/target/debug/ui` directly (`--port` to change the
port).

- **Conversations** are listed on the left and saved to `.agent/chats` after
  every turn.
- **Each turn's tool calls** sit in one card headed by a sentence — "Working
  in Word" with a running clock, then "Worked in Excel and PowerPoint ·
  1 refused · 6 steps" — with one row per call in the colour of the app it
  touched.
- **The reply is drawn as the model writes it.** What the model is thinking
  shows on one line until the text starts; the finished answer replaces the
  draft.
- **Approvals** (a `shell` call) attach to the composer with Approve and Deny.
- **The model button** opens a searchable list of every model your provider
  serves that can call tools, with context size, price and whether it can
  reason. The list comes from the provider's own `/models`, refreshed every
  15 minutes and on demand. OpenRouter is listed by default; OpenCode Zen
  appears once `AGENT_API_KEY_OPENCODE` is set. "Automatic" means the model in
  `.env`.
- **The thinking level** (auto, low, medium, high) sets how hard the model
  reasons before it answers.
- **The status menu** (top right) connects the applications named in `.env`,
  switches between system, light and dark themes, and sets contrast.
- **Runs from elsewhere show up too.** A run started from a terminal or by an
  MCP client writes to the same live log, and the console shows it as it
  happens.

The page follows each run over a server-sent event stream that resumes from
its last line if the connection drops, and polls in the meantime.

The console binds 127.0.0.1 only, and refuses any command whose `Origin` is
not its own page (and any with no `Origin`), so neither another machine nor a
web page you happen to visit can send it commands.

## The CLI

```
core\target\release\cli.exe
```

It reads one command per line and answers with lines that start with a tag:
`RECEIPT`, `ERROR`, `ANSWER`, `CONFIRM`, `STOPPED`, and so on.

### Working with the agent

| Command | Does |
|---|---|
| `say <message>` | One turn of a conversation; the transcript is kept, so the next `say` continues it. |
| `do <goal>` | A single goal, run to completion without a conversation. |
| `approve` / `deny <reason>` | Answer a `shell` call waiting for a human. |
| `model <provider> <id>` / `model auto` | Choose the model for the next turn. |
| `think low\|medium\|high\|auto` | Choose the thinking level. |
| `models <search>` | Search the providers' model lists. |
| `slots`, `config` | Show what resolved where (never the key). |
| `chat new\|list\|open <id>\|del <id>\|msgs` | Manage saved conversations. |
| `shellallow <program>` | Allow one program for `shell` in this session (the list starts empty). |

### Connecting and opening

| Command | Does |
|---|---|
| `hand <pipe> [app…]` | Connect to a running helper; with apps, it serves those apps. |
| `cdp <host:port> web` | Connect to a browser or Electron app's DevTools port. |
| `hands`, `wiring` | Show connected helpers, and what `.env` names. |
| `open <app> <path>` | Open a file through a connected helper (idempotent). |
| `attach excel <file> <sheet>`, `attach word <file>`, `attach ppt <file>` | Register a document; without `live` it uses Syn's in-memory model, which needs no Office. |
| `live <handle>` | Bind a registered handle to its application. |
| `page web <title-or-url> [unit]`, `win ui <title> [unit]` | Register a web page or a window. |
| `registry` | List open handles. |

### Operating on documents by hand

`read`, `write`, `format`, `export`, `undo` and the `struct` verbs are all
available as commands (`read excel:plan.xlsx:Sheet1 Sheet1!A1:C5`), and go
through exactly the same gates as the agent's calls.

### Controls

| Command | Does |
|---|---|
| `pause`, `resume` | Stop taking calls, and carry on. |
| `kill` | Latch the kill switch: nothing more is dispatched until a fresh session. |
| `allow <app…>` | Restrict this session to those applications. |
| `journal <path>` / `journal off` | Record every command to a file. |
| `replay <path>` | Run a recorded journal again. |

### Example

```
cli.exe
  hand hand-excel excel
  open excel C:\work\plan.xlsx
  shellallow hostname
  model openrouter vendor/model
  think high
  do Read the sheet and write its shape into the report.
```

A `shell` call stops for a human:

```
CONFIRM hostname
        reason given: To get the machine name
        respond with `approve` or `deny <reason>`
```

A program not on the allowlist is refused before anyone is asked.
