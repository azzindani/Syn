# Security

Syn lets a language model act in applications that hold a person's real
work. The model is treated as capable but not trusted: it can be wrong,
and anything it reads may have been written by someone trying to steer it.
This page describes what Syn does about that, and what remains the user's
responsibility.

## Scope

Syn is for **one user on one machine**. It runs as that user, with that user's
access to their files and applications. It has no accounts, no network
service and no multi-tenant isolation. Everything below limits what a model
can do *within* that user's session; nothing in Syn is a sandbox.

## Every call goes through the same gates

The agent loop, the CLI and every MCP client reach a document through one
function, `Runner::run`. On the way each call meets the kill switch, the app
allowlist, the VBA gate, the per-app verb table, the repeated-call gate and
the registry check (see [architecture.md](architecture.md#how-a-call-travels)).
There is no other path to a document.

- **Kill switch** (the `kill` command): stops all further
  dispatch. It cannot be undone from inside the session; a fresh session is
  needed.
- **App allowlist** (`allow excel word`, or `AGENT_MCP_APPS` for MCP):
  anything outside it is refused before it reaches a helper.
- **Repeated-call gate:** the same call on the same document three times in a
  row pauses the run until a human sends the next message.

## Tools are closed

Every tool has a closed JSON Schema (`additionalProperties: false`). Unknown
fields and unknown verbs are refused, not ignored. Every string has a length
cap, and input over a cap is refused, never silently truncated. A model
chooses an application from a fixed list; which program serves it comes only
from configuration.

## What a model reads is data

Anything read from a document, a web page, a window or a program's output
goes back to the model inside `<user_content>` with a line telling it to
treat the content as data, never as instructions. Text that looks like an
injected instruction ("ignore all previous instructions", `[system:`) adds a
warning line. Tool output is capped in length.

Tool descriptions carry their own rules — what the tool does, what it does
not do, and when to use it — and the surface is fingerprinted, so a change to
it is detectable.

## Running programs: `shell`

`shell` runs **one program** with arguments, never a shell: no pipes,
redirects, globs or metacharacters are interpreted, and no paths are
accepted. It is offered only to Syn's own agent, never over MCP, and:

- its allowlist is **empty** by default; a person adds programs for a session
  with `shellallow <program>`;
- a program not on the list is refused before anyone is asked;
- every allowed call still **stops for a human**, who sees the exact command
  line and the model's stated reason, and approves or denies it;
- output is capped and returned as untrusted data.

## VBA

`struct` `macro` can write and run VBA in an Excel workbook, which is code
running with the user's full privileges. It is **off by default**:

- it is refused unless the process was started with `AGENT_VBA=1`;
- Excel must also have *Trust access to the VBA project object model* turned
  on (Trust Center → Macro Settings);
- the workbook is copied before every `run`, because running VBA clears
  Excel's undo list and `undo` cannot take a macro back;
- a filter refuses obviously dangerous calls (`Shell`, `CreateObject`,
  `Kill`, `SendKeys`, `Declare`, …). String concatenation defeats any filter
  like this: treat enabling VBA as trusting the model with your account.

## Documents and files

- **Nothing is closed or saved** that Syn did not open itself. Office COM
  servers are single-instance per user, so quitting one would close the
  user's own documents.
- **Exports are copies**; the open document keeps its own path.
- **`open` is idempotent** and never discards unsaved work.
- **`AGENT_MCP_ROOTS`** confines which folders an MCP client may open files
  from.
- **Undo refuses** when someone else has edited the document since Syn's
  change, rather than take their work back with it.

## Local services

- **The console** binds `127.0.0.1` only, and accepts a command only when its
  `Origin` header is the console's own page. Browsers cannot forge `Origin`,
  so a web page you visit cannot post commands to it, and requests with no
  `Origin` are refused.
- **The helpers** listen on named pipes (Unix sockets off Windows) local to
  the machine.
- **The browser hand** needs a Chromium started with
  `--remote-debugging-port`. That port is unauthenticated: anything on the
  machine that can reach it controls the browser, including its logged-in
  sessions. Keep it on `127.0.0.1` and use a separate `--user-data-dir`, not
  your everyday profile.

## Secrets

API keys are read from the environment or `.env` only when a request is
sent. They are never logged, never written to a chat, a journal or the live
log, and never put in a prompt or a tool result. `.env`, `.agent/` and
`testbed/` are gitignored.

## What stays with the user

The manual pages tell models that a button which deletes, sends, pays or
signs in is the user's to press, and never to type a password. That is
guidance, not enforcement: a model with access to a window can press what it
can see. Keep the app allowlist narrow, leave VBA and `shell` off unless a
task needs them, and watch runs in the console.
