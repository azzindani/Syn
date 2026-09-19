# 05 — t3code UI digest (what a chat client should look like)

Source: `pingdotgg/t3code` (TypeScript, React 19 + Tailwind v4). 376 MB repo, so
read through the GitHub tree API rather than cloned. Nothing is ported: we have
no build toolchain and `core` is zero-dependency. What follows is the *shape*.

## Vocabulary
`thread` (a conversation), `composer` (the input area and everything attached to
it), `timeline` (the message list). Threads live in a sidebar; the timeline and
composer share a centred column.

## The five ideas worth stealing

1. **Tool calls are quiet one-line rows, not chat bubbles.**
   `flex min-h-6 items-center gap-1.5 rounded-md px-0.5 py-0.5 text-sm
   hover:bg-accent/20`, clickable to expand. A run that makes twelve tool calls
   stays readable because the calls are a thin list, not twelve blocks. This is
   the single biggest difference from a terminal transcript, where every step
   shouts at the same volume as the answer.

2. **The approval panel attaches to the composer, not the timeline.**
   `ComposerPendingApprovalPanel`: a labelled group above the input, the command
   itself in `whitespace-pre font-mono`, a `1/N` counter in `tabular-nums` when
   several are queued. The decision appears where the human's attention already
   is, and it is visibly a question rather than a message that scrolled past.

3. **Long user messages collapse** past a threshold: `max-h-44 overflow-hidden`
   with a mask-image fade and a "Show full message" button. A pasted log does not
   push the conversation off screen.

4. **Semantic colour tokens, not raw colours.** `--muted-foreground`,
   `--secondary-label`, `--border`, `--accent`, `--ring`, `--warning`,
   `--surface-raised`. Light and dark are two sets of the same names, so nothing
   downstream knows which theme is on.

5. **One centred column, `max-w-3xl`,** with the composer as a rounded (22px)
   raised card carrying a hairline outline and, in dark mode, an inset top
   highlight. That single card is most of why these apps feel like apps.

## The design system (this is the part I missed first time)

`apps/web/src/index.css` is where the look actually lives, and it is a **zinc
neutral scale with a single blue primary** — not a warm palette, and not a
colourful one. Reading the components without reading this file produces
something with the right boxes and the wrong face.

Light:
```
--background  zinc-25  oklch(99.2% 0 0)   --accent  zinc-100 #f4f4f5
--foreground  zinc-800 #27272a            --border  zinc-200 #e4e4e7
--card        #ffffff                     --input   zinc-300 #d4d4d8
--muted-foreground zinc-500 #71717a       --primary oklch(0.488 0.217 264)
--sidebar     zinc-50  #fafafa            --sidebar-row-selected #ffffff
```
Dark is the same names over `--background: #0a0a0a` (neutral-950), with
`--card` as `color-mix(in srgb, var(--background) 97%, #fff)` and the borders
as white at 6–10% rather than a lighter grey.

Metrics that matter more than they look:
- `--radius: 0.625rem` (10px) app-wide, `--control-radius: 0.5rem` (8px) for
  controls. The composer is the one exception at 22px. Mixing 9/16/20px, as I
  did, is most of what reads as amateur.
- Sidebar rows: `h-8` (32px), one line, `truncate`, 10px side padding. Not two
  lines with a timestamp — that turns a thread list into a feed reader.
- Buttons: `h-8` (32px), 8px radius; primary carries
  `inset-shadow 0 1px white/16%` over the primary fill.
- `--workspace-topbar-height: 52px`.
- Font: `-apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif`.

## From the full clone (668MB, 23,041 files)

Reading the tree rather than files picked by hand turned up the logic modules
that sit beside the components, which is where several answers actually live.

**`components/Sidebar.tsx` — thread rows have a hierarchy.** Inactive titles
render at `text-secondary-label/70` and brighten to `text-foreground` only on
hover or focus; the active row gets `bg-sidebar-row-active` and full strength.
Rendering every row at `--foreground`, as we did, is precisely what makes a
thread list read as a flat wall of equally loud text.

**`components/AppSidebarLayout.tsx` — the sidebar is furniture, not a fixture.**
`collapsible="offcanvas"`, resizable with the width persisted to localStorage,
and the toggle is a *fixed* control in the titlebar strip
(`left: var(--workspace-controls-left)`, `height: var(--workspace-topbar-height)`,
`z-50`) so it survives the panel sliding away. The shell is `h-dvh`, not
`100vh`: `vh` is wrong wherever browser chrome moves.

**`components/threadSidebarWidth.ts`** — default 16rem, minimum 13rem, and the
main content keeps a 40rem floor, so the maximum is `viewport - 40rem`.

**`timestampFormat.ts`** — terse relative labels (`just now`, `5m ago`), and day
boundaries are whole *local calendar days*, rounded so 23- and 25-hour DST days
still count as one. A 24-hour window files last night's 11pm message under
"Today" until 11pm tonight.

**`ui/empty.tsx`** — `flex-1` + `justify-center`, `text-balance`, and a 36px
bordered icon badge. An empty screen without one reads as a failed load.

**`apps/desktop`** — Electron wrapping the same `apps/web` build. There is no
separate desktop UI; the app is a native window around the same page. That is
the shape our Tauri shell should take too.

## What we take, given no React and no bundler
Layout, information architecture, the collapsed-tool-row idea, the composer-
attached approval, and the token vocabulary. All of it is plain CSS and a few
hundred lines of vanilla JS in one file, served by the `ui` binary.

## What we do NOT take
Tailwind utility soup in markup, the glass/backdrop-filter clip-path work in
`ComposerSurface.tsx` (hundreds of characters of `clip-path: shape()` for one
seam), and the React component split. Our page has one job and no build step.
