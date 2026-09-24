# 09 — t3code, the visual layer

Source: `pingdotgg/t3code` at `e4eb9977` (2026-09-23), cloned to
`.tmp/repos/t3code` (gitignored). MIT licensed; ideas are taken, no code or
asset is copied.

`DIGEST-08` read t3code for how a long run stays *legible*: label tables,
grouping, banner priority, the minimap, the contrast dial. All of that was
built, and the console still read as plain, because none of it is what a
person sees first. This digest is the other half: what makes t3code look
like an application rather than a log viewer. The page that follows from it
is `widget/index.html`; the specs that hold it are `tests/ui/render.spec.mjs`
and `tests/ui/shell.spec.mjs`.

---

## 1. Every row has a picture

`chat/MessagesTimeline.tsx` never renders a bare tool line.
`LiveActivityContent` puts a 24px icon slot in front of every label, filled
by `ToolActivityIconView`: a favicon for a website, an app logo for a
native app, otherwise a lucide glyph chosen by `toolGroupSummaryIconName`
(`eye` for reads, `square-pen` for edits, `terminal` for commands). Muted at
70% opacity so the label stays the thing you read.

**Taken:** every `.act` row carries an app tile — Excel green, Word blue,
PowerPoint orange, browser, window, terminal — keyed off the `app` and
`tool` fields core already sends in `RECEIPT step` and `LABEL`. Never off a
verb: `console_contract.rs` forbids the page naming one, and an app is the
handle's own prefix, not vocabulary. The letter in the tile is a label, not
a logo.

## 2. A group is one sentence, and it has a tense

`WorkGroupToggleTimelineRow` is a single line: icon, the summary from
`summarizeToolGroup`, a timestamp. `WorkingTimelineRow` above a running
turn says *"Working for 0:42"* with a self-ticking `WorkingTimer` so only
that span re-renders.

**Taken:** each turn's calls sit in one card whose header is a sentence —
*"Working in Word"* in sky with a running clock while the run is live,
*"Worked in Excel and PowerPoint"* with a step count once it settles, and
an amber or red pill for anything refused, failed or stopped. A finished
card with more than eight rows folds to that sentence. A turn that touched
nothing leaves no card at all.

## 3. Light through the running row

`@utility live-tool-shine` in `index.css`: the active row's label is
`background-clip:text` over a moving gradient, `steps(30)` so it is thirty
frames a pass rather than one per vsync, and off under
`prefers-reduced-motion`.

**Taken as is.** It says "this one, now" without a spinner per row.

## 4. Status has one colour everywhere

`Sidebar.tsx` fixes the hues and says why: *"Status hues follow the
system-wide convention … (amber approval, indigo input, sky working) so a
thread reads the same color everywhere it surfaces."*

**Taken:** sky for working (the live card, the activity banner, the pulsing
dot on the sidebar row of the chat that is running), amber for waiting on a
human (the approval card, refusals), emerald for done, red for a stopped
run.

## 5. The first screen is a sentence

`chat/DraftHeroHeadline.tsx`: an empty draft is a large, centred, one-line
question — *"What should we build in `project`?"* — at `text-2xl sm:text-3xl
tracking-tight`, with the project as an inline picker.

**Taken:** *"What should Syn do?"* over a brand mark, one line of what it
can reach, four suggestion cards that go **into the composer, not sent**,
and a row of the apps it works with. An empty screen that only says "empty"
reads as a loading failure.

## 6. The composer is glass

`chat/ComposerSurface.tsx`: a 22px card whose background is
`color-mix(surface 80%, transparent)` under `backdrop-filter:blur(12px)
saturate(1.14)`, an `rgb(0 0 0/8%)` hairline, `0 12px 28px -18px` shadow in
light, an inset top highlight in dark, and an `@supports not` fallback to
the opaque surface.

**Taken**, with a soft focus ring in the primary, and a fade above the dock
so text scrolls under it rather than being cut.

## 7. Geometry and type

`--glass-blur`, `--glass-opacity`, `--control-radius` and friends are
semantic tokens, *"so sidebar, palette, tooltip, and toolbar controls
cannot quietly drift apart."* Radii step from one `--radius`. Numbers are
`tabular-nums` wherever they change.

**Taken:** the same naming, `tabular-nums` on the timer, the budget and
table figures, and a system font stack that prefers Inter or Segoe UI
Variable when installed. No web font: the page is served from memory and
must work offline.

## 8. Theme is a choice

t3code has a full theme system (`themePalette.ts`, VS Code theme import).
**Taken, small:** system, light or dark, pinned per viewer in the status
menu, resolved to one `data-theme` attribute before first paint so a dark
user never sees a white flash. "System" follows the OS live. The contrast
dial `DIGEST-08` §8 built now has a visible slider beside it.

---

## Deliberately not taken

- **Lucide as a dependency.** The glyphs are drawn inline in the same
  idiom; the page stays one file with no requests.
- **Favicons and remote logos** (`toolActivityFaviconUrl`). A tool result
  is untrusted; the tiles are fixed and local.
- **The mobile composer view transitions** and draft-hero morph. Nice,
  costly, and not what makes the page read better.
- **Projects, branches, PR chips, the diff panel.** t3code is a coding
  tool; none of that has a meaning here.

## What was read

**In full:** `NoActiveThreadState.tsx` (35), `chat/ComposerSurface.tsx`
(99), `chat/DraftHeroHeadline.tsx` (258).

**The parts that decide something:** `chat/MessagesTimeline.tsx` — the
working row and timer, `WorkGroupSection`, `LiveActivityRow`,
`LiveActivityContent`, `WorkGroupToggleTimelineRow`, `ToolActivityIconView`,
`WorkEntryIcon` (~450 of ~5,000 lines); `index.css` — `:root` geometry and
glass tokens, `@theme`, the status keyframes, `live-tool-shine` (~350 of
2,205); `Sidebar.tsx` — the status hues (~40 of ~5,000);
`ThreadStatusIndicators.tsx` — `ThreadStatusLabel` (~50 of 611).

**Not read:** `ChatComposer.tsx`, `ChatView.tsx`, the settings tree, the
model picker, the command palette, mobile.
