# Browser testbed

A small local web site for testing browser control: forms, a single-page
app, dialogs, overlays, shadow DOM, frames, new tabs, downloads, slow and
broken responses, and a page that tries to give the agent orders. Standard
library Python only. Every page is plain HTML with inline CSS and JS, and
nothing is fetched from the network, so a run is the same every time.

```
python tests/browser/site.py          # serves on a free port, prints the URL
python tests/browser/site.py 8080     # or on a port you choose
```

From code, `start(port=0)` returns `(server, base_url)`: the server is
already serving on a daemon thread, bound to `127.0.0.1`, and
`server.shutdown()` stops it and closes the socket.

`site.py` is also the name of a standard-library module, so `import site`
gives you that one. Load this file by path
(`importlib.util.spec_from_file_location`), as `test_site.py` does.

```
python tests/browser/test_site.py                  # about 6 s (/slow waits 3)
python -m unittest tests.browser.test_site         # from the repo root
```

It is not under `tests/` top level, so `unittest discover -s tests` does not
pick it up.

## Pages

Ids are exact. Behaviour is deterministic: the only timers are `/spa`
(800 ms, 500 ms) and `/slow` (3 s). Pages with a `#out` start it at `none`.

| path | title | elements and behaviour |
|---|---|---|
| `/` | Testbed home | `h1` "Syn browser testbed"; links `to-form`, `to-spa`, `to-dialogs`, `to-overlay`, `to-editor`, `to-frames`, `to-newtab`, `to-table`, `to-long`, `to-redirect`, `to-missing`, `to-slow`, `to-download`, `to-inject`, `to-shadow`, `to-hover`, and for the rest `to-server-error`, `to-big`, `to-cookie`, `to-title-change` |
| `/form` | Contact form | POST form to `/submit`: `name` (label "Your name"), `email` (placeholder only, no label), `pw` (password), `card` (`autocomplete="cc-number"`, no label), `color` select (red/green/blue, text Red/Green/Blue), `agree` checkbox (label "I agree"), `size-s` and `size-l` radios, `msg` textarea; buttons `send` (submit), `reset`, `delete` ("Delete account", type=button, sets `status` to `deleted`); `status` starts `idle` |
| `/submit` (POST) | Thanks | `h1` "Thanks, NAME"; one `li` `name=value` per posted field, HTML-escaped. A GET is a 405 |
| `/spa` | Single page app, then SPA ready | `app` reads "Loading..."; at 800 ms it holds `items` (3 `li`), `more`, and `count` ("3 items"). `more` adds 3 `li` after 500 ms and sets `count` to "6 items"; clicks while a load is pending are ignored |
| `/dialogs` | Dialogs | `do-alert` (alert "hello from alert", then `out` = `alert closed`), `do-confirm` ("really?", `out` = `confirm:true` or `confirm:false`), `do-prompt` ("your name?", `out` = `prompt:<value>`, `prompt:null` if cancelled); `dirty` text input: once typed in, leaving the page asks first (beforeunload); `leave` links to `/form` |
| `/overlay` | Overlay | `veil` is a fixed transparent full-width div over `covered`; `dismiss` (not covered) removes it; `covered` sets `out` to `covered clicked`; `needs-real` sets `out` to `trusted click` only for a real pointerdown + pointerup (`isTrusted`), otherwise `untrusted click ignored` (so `element.click()` and a keyboard activation are ignored) |
| `/editor` | Editor | `rich` (contenteditable); `ctl` controlled input: `state` follows only `input` events and the value is reset to it on `change`/`blur`, `mirror` shows `state`; `auto` shows `sugg` (`li`, prefix matches of apple, apricot, avocado, banana, blueberry, cherry) only on keydown/keyup; `enter` sets `out` to `enter pressed` on Enter |
| `/frames` | Frames | same-origin iframe `inner` (`/frame-inner`, title "inner frame"); `outer-btn` sets `outer-out` to `outer clicked` |
| `/frame-inner` | Inner | `inner-btn` sets `inner-out` to `inner clicked` |
| `/shadow` | Shadow | `<x-card id="card">` with an open shadow root holding `shadow-btn` and `shadow-out` (`shadow clicked`); light-DOM `light-btn` sets `light-out` to `light clicked` |
| `/newtab` | New tab | `blank` (`target="_blank"` to `/form`); `winopen` runs `window.open('/spa')` |
| `/table?page=N` | Table page N | `data` table, header Name/Qty/Price, 10 rows a page, 35 rows over 4 pages: row n is `Item n`, qty `n*3 % 17`, price `n*1.25` to 2 places. `next` (not on page 4) and `prev` (not on page 1). Default page 1; out-of-range pages are clamped |
| `/long` | Long page | `s1` to `s60`, each 200 px tall; `lazy` at the bottom is empty until it scrolls into view, then reads `lazy loaded` |
| `/redirect` | | 302 to `/form` |
| `/missing` and any unknown path | Not found | 404, `h1` "404 Not found" |
| `/slow` | Slow page | waits 3 s, then `h1` "Slow done" |
| `/server-error` | Server error | 500 |
| `/download` | | `Content-Disposition: attachment; filename="report.txt"`, body `quarterly numbers` |
| `/inject` | Notes | `note` holds an instruction aimed at the agent (to be treated as page text); `secret` is `display:none`; `decoy` is `aria-hidden="true"` |
| `/hover` | Hover menu | `submenu` (with `sub-item`, link to `/form`) is `display:none` until a real pointer enters `menu-host` (mouseenter/mouseleave, not CSS `:hover`) |
| `/big` | Big page | about 300 KB of visible text in 2600 paragraphs, ending with `big-end` ("END OF BIG PAGE") |
| `/cookie` | Cookie | first visit sets `seen=1` (a year) and `state` reads `first visit`; with the cookie, `state` reads `welcome back` |
| `/title-change` | Before | `rename` sets the title to `After`; `go` links to `/form` |
| `/favicon.ico` | | 204, so it never shows as a failed request |

Every response is sent with `Cache-Control: no-store`.
