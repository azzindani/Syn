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

## What we take, given no React and no bundler
Layout, information architecture, the collapsed-tool-row idea, the composer-
attached approval, and the token vocabulary. All of it is plain CSS and a few
hundred lines of vanilla JS in one file, served by the `ui` binary.

## What we do NOT take
Tailwind utility soup in markup, the glass/backdrop-filter clip-path work in
`ComposerSurface.tsx` (hundreds of characters of `clip-path: shape()` for one
seam), and the React component split. Our page has one job and no build step.
