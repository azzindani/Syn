# 09 — Splitting the harness from the tools

Date: 2026-09-20
Status: scenario, not a decision. Nothing here is scheduled.

A plan for the day this becomes two things: an **MCP server** that drives the
applications on this machine, and a **harness** that thinks. Written now,
while the seam is still cheap to move, so that the decision is made once
rather than drifted into.

## 1. Why it would be worth doing

Three reasons, in the order they are likely to bite.

**Someone else's brain.** The hands are the hard part and the rare part.
Late-bound COM into a live Office instance, a UIA tree, a CDP session — that
is a year of other people's debugging (see `runbook-windows.md` §0). The
agent loop is the replaceable part: Claude Code, Claude Desktop, OpenCode and
Cursor all ship one, and all of them speak MCP. As an MCP server, Syn's hands
become usable from a brain nobody here has to build or pay for.

**Two release trains.** The hands change when Office changes — rarely, and
for reasons outside this repo. The loop changes every time a model does
something surprising, which this week was three times. Those cadences do not
belong in one version number.

**A boundary you can reason about.** Today the only thing that can drive a
live document is our own loop, so "is this safe" is answered by reading one
codebase. After a split it is answered by the server's gates alone, which is
both harder and more honest.

## 2. Where the seam goes

The refactor of 2026-09-20 already drew most of it.

| Today | Goes to |
|---|---|
| `tools` — the 6 ops + `shell` | **server**, except `shell` (see §3) |
| `ops`, `runner`, `queue`, `guard`, `security`, `snapshots` | **server** — the gates must hold for any caller |
| `hand`, `cdp`, `ws`, `vfs`, `ooxml`, `paths` | **server** |
| `sidecar-csharp/Host`, `sidecar-csharp/Uia` | **server** |
| `mcpgate` | **server** — becomes the front door rather than a side door |
| `agent`, `looptools` (`plan`), `provider`, `router` | **harness** |
| `chats`, `live`, `bin/ui`, `widget/` | **harness** |
| `bus`/`relay`, `sessions`, `memory` | **contested** — see §3 |
| `manual` | **both, split by audience** — see §3 |

The test for which side something belongs on: *would it still make sense if
the caller were Claude Desktop?* A slicer needs its pivot first whoever asks.
A step budget does not exist unless you own the loop.

## 3. The five decisions that are not obvious

**`shell` does not go.** It is not a document op, it always stops for a
human, and its allowlist is empty by default. An MCP server that can run
programs is a different and much larger security object than one that can
edit a spreadsheet. It stays harness-side, where the approval prompt and the
human are.

**The gates go with the tools, not the loop.** Kill switch, app allowlist,
handle registry, per-file snapshots — all server-side. Today they sit in
`Runner::pump` and are reached because our loop dispatches through it; after
a split they have to be reached because *the op* goes through them. The
doom-loop gate is the exception: "the last three calls were identical" is a
property of a conversation, so it stays with the loop.

**`manual` splits by audience, which is the cleanest evidence the refactor
was right.** The `excel`, `word` and `powerpoint` pages describe verbs and
belong to the server — an MCP server should document itself, and those pages
are already free of anything about this project's test fixture. The `loop`
page is about a budget, a plan and prose ending a turn. None of that exists
for another client. It stays harness-side.

**The event feed is the hard one.** `step.start` / `step.live` / `step.done`
is what makes a run watchable, and it is the answer to the headless-blind
problem this project was started over (`07-live-orchestration.md`). MCP is
request/response with server notifications; a per-op progress stream is
expressible but is not what most clients render. Options, none free:

1. Server emits MCP notifications; clients that ignore them lose the live
   view, and ours does not.
2. Keep the relay as a second channel the console connects to directly. Two
   transports to keep alive.
3. Accept that only Syn's own harness gets the live view, and other clients
   get plain tool results.

Option 3 is honest and cheapest, and it should be stated as a limitation
rather than discovered by a user.

**Sessions and memory stay with the loop.** A session is a conversation. The
server should be stateless about conversations and stateful only about
documents.

## 4. What is actually in the way today

Not architecture — two concrete defects.

**`mcpgate` publishes a different, worse surface than the loop sees.** It
carries its own hand-written list:

    ("write",  "Write grid cells or a paragraph. Args: handle, selector, payload.")
    ("struct", "Structural verbs: addSheet/addTable/xfer/chart. ...")

against `tools::TOOLS`, which calls those arguments `values`, and those verbs
`insertTable` and `transfer`. **An MCP client is being told argument names
that do not exist.** It also ships no JSON Schema at all — no
`inputSchema`, so none of the closed-schema work (`additionalProperties:
false`, caps, refuse-don't-truncate) reaches an external caller. Every
security property argued for in `08-production-grade.md` is in-process only.

Fix before anything else: `mcpgate` builds its list from `tools::TOOLS`,
with `tools::spec_json` reshaped for MCP's `inputSchema`. One source of
truth. This is worth doing whether or not the split ever happens.

**The gates are reachable only through our dispatch path.** `mcpgate`'s
mutating ops queue an `office-rpc/1` envelope; the in-process path goes
through `Runner`. Two roads to the same document, one of which has the
guard-rails. That has to become one road before an external client is
trusted on it.

## 5. A path that is reversible at every step

Nothing here requires committing to the split.

- **Phase 0 — done.** `tools` / `looptools` / `surface`: world tools apart
  from loop services, merged only at the wire, fingerprint over the world
  tools only.
- **Phase 1 — one surface.** `mcpgate` generated from `tools::TOOLS`, with
  real schemas. Closes the divergence above. *Useful on its own.*
- **Phase 2 — one road.** Every op reaches a document through the same
  gated dispatch, whoever called it. *Useful on its own.*
- **Phase 3 — one workspace, several crates.** `syn-ops`, `syn-hands`,
  `syn-mcp`, `syn-agent`, `syn-ui` in this repo. The compiler starts
  enforcing the seam; no release or packaging changes. Cheap, and trivially
  undone.
- **Phase 4 — two repos.** Only with a reason from §6. This is the step
  that costs: two release trains, a versioned protocol between them, and
  contributors who have to clone twice.

Phases 1–3 are worth doing on their own merits. Phase 4 is the only one that
is actually a split, and it should wait for a user who is not us.

## 6. What would make it right, and what would make it a mistake

**Do it when** someone wants the hands without the brain — a person driving
Office from Claude Desktop, or a second harness worth running; or when the
sidecars stabilise while the loop is still changing weekly; or when the
security story has to be defensible to somebody other than its author.

**Do not do it** to be tidy. Right now there is one user, one machine and one
loop, and a two-repo project with a versioned wire protocol between halves is
a tax paid every day for a benefit nobody has asked for yet. The capability
runs are gated on the loop, not on the boundary.

**The signal to watch:** how often a change has to touch both sides at once.
While `agent.rs` and `tools.rs` keep changing together — as they did for the
manual, the plan and the status note — the seam is not where this document
says it is, and splitting would just move the friction into a protocol
version number.

## 7. What stays true either way

The six primitives are the product. `read/write/format/struct/export/undo`
over `app:file:unit` handles, one vocabulary across COM, CDP and UIA, no
plugins. That claim is what makes the hands worth publishing, and it is
unchanged by which process they run in.
