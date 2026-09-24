# 08 — t3code as a frontend (UI digest)

Source: `pingdotgg/t3code` at `803f94e` (2026-09-18), cloned to
`.tmp/repos/t3code`. 78,165 lines of `.tsx` in `apps/web` alone.

**This supersedes `DIGEST-05`.** That one was read through the GitHub tree API
without a clone, was 111 lines, and got the broad strokes right — quiet tool
rows, approval attached to the composer, collapsing long user messages,
semantic tokens, one `max-w-3xl` column. All five hold up. What it missed is
everything about *how a long agent run stays legible*, which is the only
reason we want a UI at all. §9 says what was read.

The goal, in the user's words, is "a UI that a regular human can read." t3code
has solved that problem specifically, and the solution is not in the
components. It is in three pure-logic modules that the components merely
render.

---

## 1. The load-bearing idea: tool names never reach the screen

`packages/client-runtime/src/work-log/presentation.ts` holds a table mapping a
tool name to **four grammatical forms**:

```ts
const T3_MCP_TOOL_LABELS: Record<string,
  readonly [action: string, running: string, completed: string, detail: string]> = {
  link_pull_request: ["Link", "Linking", "Linked", "a pull request"],
  preview_snapshot:  ["Take a snapshot of", "Taking a snapshot of",
                      "Took a snapshot of", "the preview page"],
  device_screenshot: ["Take a screenshot of", "Taking a screenshot of",
                      "Took a screenshot of", "the device"],
  …
}
```

and picks the form from the call's lifecycle status:

| status | label |
|---|---|
| `inProgress` | *Linking* |
| `completed` | *Linked* |
| `failed` | *Failed to link* |
| `declined` | *Declined to link* |
| `stopped` | *Stopped linking* |

Then `displayName = ${verb} ${target}`, where `target` is the **specific**
noun when the arguments give one (`PR #482`) and the generic `detail`
(`a pull request`) when they do not.

A human never sees `link_pull_request({url: "…"})`. They see
*"Linked PR #482"*, and while it runs, *"Linking PR #482"*.

For us this maps exactly onto the six primitives. `struct` with
`verb: "addChart"` on `excel:plan.xlsx:Sheet1` is
`["Add", "Adding", "Added", "a chart"]` with target `to Sheet1`. `write` with
a selector is `["Write", "Writing", "Wrote", "cells"]` with target
`Sheet1!G1:I1`. The table is about twenty lines of data for our whole surface,
and it converts the transcript from a machine log into English.

Two things about where it lives matter as much as what it does. It is a **pure
function in a shared package**, consumed by both the web and the mobile
client, and it has a 712-line test file next to it. Label logic is not
allowed into a component.

---

## 2. Many calls collapse into one sentence

`summarizeToolGroup()` takes a run of tool entries and returns one line.
Each entry is classified by `toolGroupAction()` into
`read | edit | command | code-search | search | browser | device | link-pr |
unlink-pr | list-prs | other | update`, grouped, counted, and phrased:

```
Read 4 files, changed 2 files, and ran 3 commands
```

with proper pluralisation, an Oxford comma, and the second and later clauses
lower-cased so the whole thing reads as one sentence.

The counting rule for edits is the detail worth stealing: `toolGroupActionCount`
counts **distinct changed files**, not calls. Six edits to two files reads
*"Changed 2 files"*, because that is what happened from the human's point of
view. Everything else counts calls.

Our equivalent: *"Read 3 ranges, wrote 12 cells, and added a chart"* for a
stretch that was forty `office-rpc/1` envelopes.

---

## 3. A failed command is not an error

```ts
export function workEntrySignalsSevereFailure(entry: WorkLogEntry): boolean {
  return entry.sourceActivityKind === "runtime.error"
      || entry.sourceActivityKind?.endsWith(".failed") === true
}
```

The comment above it is explicit: *"runtime errors and orchestration `*.failed`
activities mean the turn or a core side effect broke, not that a command
exited nonzero."* A nonzero exit gets an ordinary tool-failure treatment; only
a broken turn gets the red one.

This is the same distinction our capability grader got wrong, in the same
direction. We scored a refused tool call as the model giving up, and it took
five instrument fixes to separate "the tool said no" from "the harness broke."
t3code encodes that distinction in the type of the event, not in a heuristic
over the text.

There is a matching function for the ambiguous case —
`workEntryIndicatesToolNeutralStatus` — for a tool-like row with neither clear
success nor failure. Those are hidden from collapsed groups, with one carve-out
whose comment reads: *"Spawn CTA rows are never neutral-hidden: mid-run they
derive from `task.progress` and the neutral filter was swallowing them exactly
while the fleet ran — the one moment they matter most."*

---

## 4. The timeline is a projection, not a message list

`apps/web/src/session-logic.ts` (1,750 lines) derives everything the timeline
renders. A `TimelineEntry` is a tagged union of three kinds:

```ts
| { kind: "message";       message: ChatMessage }
| { kind: "proposed-plan"; proposedPlan: ProposedPlan }
| { kind: "work";          entry: WorkLogEntry }
```

merged by `createdAt`. A `WorkLogEntry` carries `tone: "thinking" | "tool" |
"info" | "error"` — the visual register — plus `toolLifecycleStatus`,
`changedFiles`, `command`, `turnId`, and an optional `toolCallId` described as
*"stable provider identity across in-progress and completed lifecycle
updates"*, which is how a row updates in place instead of a new row appearing
when the call finishes.

The tool itself can declare its own presentation (`toolPresentation.ts`):
`toolSurface: "browser" | "computer"`, and a `toolIcon` that is one of
`{_tag: "website", pageUrl, faviconUrl, faviconUrlDark}`, `{_tag:
"native-app", app}`, `{_tag: "themed-logo", logoUrl, logoUrlDark}`. Every URL
in there is validated through `new URL()` with a protocol allow-list and a
length cap before it is rendered — a tool result is untrusted input.

For us the natural use is obvious: an Excel row carries the Excel icon, a Word
row carries Word's, and the timeline stops being a wall of identical lines.

---

## 5. The minimap — the thing DIGEST-05 missed entirely

`MessagesTimeline.tsx` renders a strip in the **left gutter**: one 2px tick per
user message, hidden at `opacity-0` until the pointer enters, tapering by
distance from the cursor (`w-6` on the active tick, then `w-4`, `w-2.5`,
`w-2`), with a hover card showing that message's text, and full keyboard
navigation (↑ ↓ Home End Enter).

It is gated on `[@media(pointer:fine)]` and on there being a real gutter — if
the window is too narrow for the strip to sit outside the centred
`max-w-3xl` column, it goes `pointer-events-none` rather than overlaying the
text. It only appears at all past `TIMELINE_MINIMAP_MIN_ITEMS = 2`.

A three-hundred-step run is unreadable by scrolling. This is the affordance
that makes it navigable, and it costs one absolutely-positioned strip.

---

## 6. Everything the run needs to say, in priority order

`ComposerBannerStack.tsx`. Banners above the input carry a priority:

```ts
function bannerPriority(item) {
  if (item.priority === "activity") return 0
  if (item.priority === "urgent" || item.variant === "error"
      || item.variant === "warning") return 1
  return 2
}
```

with the comment: *"Activity stays attached. Urgency and severity only order
the notices behind it."* **What is happening right now outranks what went
wrong.** Only the front banner is shown; the rest collapse behind a peek
control that expands, with focus moved to the first control inside on expand
and back to the peek on collapse.

The pieces that feed it are each their own component: `ComposerActivityStatus`
(a spinner and a phase label), `ComposerPendingApprovalPanel`,
`ComposerPendingUserInputPanel`, `ThreadErrorBanner`, `ProviderStatusBanner`,
`ComposerUsageLimits`, `ComposerPlanFollowUpBanner`, `WorktreeSetupCard`.

We have the same problem — a run can simultaneously be retrying, waiting for a
shell approval, over budget, and reporting that Word closed — and we currently
have nowhere to put any of it.

### The approval panel, exactly

`ComposerPendingApprovalPanel` is 66 lines. A named label per kind
(*Command approval*, *File read approval*, *App permission approval*, *File
change approval*, *App access approval*) in `text-warning`, the app name
truncated beside it, a `1/N` counter in `tabular-nums` pushed right when
several are queued, and the detail in a focusable, scrollable `max-h-20` box
— `whitespace-pre font-mono` for a command, `whitespace-pre-wrap font-sans`
for prose. Every kind also gets a distinct `aria-label`.

The detail box is `tabIndex={0}` so a keyboard user can scroll a long command
before deciding. That is the level of care the decision point deserves, and it
is the single most important widget in our UI too: `shell` always stops for a
human.

---

## 7. Showing the budget honestly

`ContextWindowMeter` sits in the composer footer and renders from real usage
in the thread's activities. Two details:

`formatContextWindowCompactionMessage` prefers
`"Compacts automatically at 150,000 tokens."` over
`"Context compacts automatically when needed."` — a number when there is one,
and either way it tells the human **what will happen**, not just where they
are.

`shouldReserveContextWindowMeter` holds the meter's slot empty while the
thread detail loads, so the attach button next to it does not jump when the
meter mounts. The comment explains the tri-state: a provider *known* not to
report usage skips the reservation; an unknown provider reserves.

Our version is the step budget and the model spend, and we have exactly the
same jump-on-load problem waiting for us.

---

## 8. The design system, properly this time

DIGEST-05 said "semantic colour tokens." It is three layers, and the middle
one is the interesting part.

1. **Raw theme tokens** per theme: `--foreground`, `--muted-foreground`,
   `--border`, `--accent`, `--warning`, `--surface-raised`, `--message`,
   `--sidebar-row-hover`, …
2. **Derived contrast tokens.** Every raw token has a `--contrast-*` twin:

   ```css
   --contrast-foreground: color-mix(in oklab,
     color-mix(in oklab, var(--foreground) var(--appearance-contrast-base), var(--background)),
     var(--appearance-contrast-target) var(--appearance-contrast-boost));
   ```
3. **Tailwind's `--color-*` maps to the contrast layer**, never to the raw one:
   `--color-foreground: var(--contrast-foreground)`.

One function drives all of it:

```ts
export function applyAppearanceContrast(root, contrast) {
  root.style.setProperty("--appearance-contrast-base",  `${Math.min(contrast, 100)}%`)
  root.style.setProperty("--appearance-contrast-boost", `${Math.max(contrast - 100, 0)}%`)
  root.style.setProperty("--appearance-contrast-border-boost", `${Math.max(contrast - 100, 0) / 4}%`)
}
```

Below 100 the colour fades toward the background; above 100 it is pushed
toward `--appearance-contrast-target`, which is `black` in light mode and
`white` in dark. Borders get a quarter of the boost so they firm up without
turning into hard lines. **One accessibility slider, zero component
awareness.** Mixing happens in `oklab` for foregrounds (perceptually even) and
`srgb` for borders and inputs.

Geometry gets the same treatment, with an explicit reason in the file:
`--control-radius`, `--sidebar-content-inset`, `--sidebar-control-gap`,
`--command-shell-inset`, `--floating-content-inset`, `--workspace-topbar-height`
— *"Keep these values semantic so sidebar, palette, tooltip, and toolbar
controls cannot quietly drift apart."*

Two utilities worth copying whole: `surface-glass` (a `color-mix` background
plus `backdrop-filter`, with an `@supports not` fallback to an opaque
background, so a browser without backdrop filters gets a readable panel rather
than a transparent one), and `alert-glass`, which is the same thing tinted 4%
by `data-variant`.

Even the animations are budgeted. The skeleton pulse carries this:

> *"one opacity pulse per container, stepped so however many bars sit under
> it, the compositor draws a handful of discrete frames per cycle rather than
> one per vsync — which on a 120Hz display is the difference between ~14 and
> ~288 updates."*

`animation-timing-function: steps(4)`. A loading animation with a frame
budget.

---

## 9. Message rendering

- **User messages** are right-aligned bubbles, `max-w-[80%] rounded-2xl
  bg-message p-3 text-message-foreground`. **Assistant messages have no
  bubble** and run the full column width. The asymmetry is the whole visual
  grammar: one participant is quoted, the other is speaking.
- **Queued messages** (typed while the agent is still working) render in the
  same place with `border border-dashed` and a clock icon, with send-now and
  cancel buttons. Not a disabled input — a visible queue.
- **A compaction boundary** renders as a horizontal rule with a centred
  `Minimize2Icon` and a label, in `text-muted-foreground text-xs`. The
  conversation visibly has a seam, rather than history silently disappearing.
- `ChatMarkdown.tsx` sanitises with an explicit schema, extracts a
  `title=`/`filename=` from a fence's info string to label code blocks,
  renders GitHub alert syntax, detects standalone images for gallery
  treatment, and caches syntax highlighting with a **bounded** cache — 500
  entries or 50 MB, whichever comes first. A long transcript of highlighted
  diffs is a memory leak otherwise.

---

## 10. The sidebar is a state machine over threads

Threads are `draft | running | settled | snoozed`, plus pinned, with sections
that can be collapsed and dragged. Rows come in two densities (`sidebar-row-slim`
and `sidebar-row-card`).

Three details worth the copy:

- `WorkingDuration` is *"self-ticking so only this span re-renders each second,
  not the whole row"* — a `setInterval` inside the leaf, and `tabular-nums` so
  the digits do not jitter the layout.
- `settledTimeLabel` and the settled sort order both go through
  `resolveSettledThreadTimestamp`, *"so label and order can't disagree."*
- `JumpHintBadge` is an absolutely-positioned overlay, not an inline slot,
  because *"holding ⌘ used to blank out 'Working'"* — a hint must not displace
  live status.

---

## 11. What to take, in order

| # | Item | Why it is first or last |
|---|---|---|
| 1 | The four-form tool label table | Converts our transcript to English. ~20 lines of data. Nothing else changes as much for as little. |
| 2 | `severeFailure` vs ordinary tool failure | We have already been burned by conflating these, in the grader. |
| 3 | Approval panel attached to the composer | `shell` always stops for a human; this is where that decision belongs. |
| 4 | Group summarisation | Makes a 300-step run scannable. |
| 5 | The three-layer contrast token system | Cheap at the start, near-impossible to retrofit. |
| 6 | Banner stack with activity-outranks-error priority | We have four things to say at once and nowhere to say them. |
| 7 | Timeline minimap | The affordance that makes a long run navigable. |
| 8 | Context/step budget meter with a reserved slot | Says what will happen, not just where we are. |
| 9 | Tool-declared icons and surfaces | Excel rows look like Excel. Validate every URL. |
| 10 | Bounded highlight cache | Only once transcripts get long. |

Items 1–4 are pure logic and testable with no renderer at all — they could be
written in Rust in `core` and exposed over the existing event feed, which is
where they belong given `split-plan.md`: labels describe *world tools*, and a label
table is documentation of the surface as much as it is presentation.

---

## 12. Coverage of this digest

**Read in full:** `work-log/toolPresentation.ts` (123),
`chat/ComposerPendingApprovalPanel.tsx` (66),
`chat/ComposerActivityStatus.tsx` (23), `appearanceContrast.ts` (10).

**Read in the parts that decide something:**
`work-log/presentation.ts` (~350 of 700 — the label table, status→verb
mapping, `toolGroupAction`, `toolGroupActionCount`, `toolGroupActionLabel`,
`summarizeToolGroup`), `session-logic.ts` (~200 of 1,750 — `WorkLogEntry`,
`TimelineEntry`, the failure predicates, `isLatestTurnSettled`),
`index.css` (~450 of 2,224 — `:root` geometry, `@theme inline`, the
`--contrast-*` derivations, `surface-glass`, `alert-glass`, the keyframes),
`chat/MessagesTimeline.tsx` (~400 of 5,061 — the minimap, the column, user
bubbles, queued rows, the compaction divider),
`chat/MessagesTimeline.logic.ts` (~120 of 1,675 — label resolution and
grouping entry points), `chat/ComposerBannerStack.tsx` (~100 of 337 — the
priority function and the collapse behaviour),
`chat/ContextWindowMeter.logic.ts` (137, read in full but it is mostly
Claude-specific resume logic), `Sidebar.tsx` (~120 of 4,986 — time labels,
`WorkingDuration`, `JumpHintBadge`, the row-variant test ids),
`ChatMarkdown.tsx` (~60 of 3,363 — the plugin list, sanitize schema, cache
bounds, fence metadata).

**Not read:** `ChatView.tsx` (10,458 — the container, streaming subscription
and scroll management), `chat/ChatComposer.tsx` (7,011), the whole
`settings/` tree (~12,000), `apps/mobile` (~15,000), `CommandPalette.tsx`
(3,040), `pullRequest/`, `DiffPanel.tsx`, `ThreadTerminalDrawer.tsx`, and the
~120 `*.test.ts(x)` files beside the modules above.

`ChatView.tsx` is the significant gap. It owns how a streaming token arrives
and whether the view stays pinned to the bottom — and "does the view follow
the run without fighting the user's scroll" is a question this digest cannot
answer. `chat/timelineScrollAnchoring.ts` and `chat/pageScrollController.ts`
exist and were not opened; they are the first things to read next.
