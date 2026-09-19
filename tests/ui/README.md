# UI tests

Playwright against the real console: it starts `ui.exe` on port 7799 (not the
7777 you browse), drives the page, and stops it again.

```
cd core && cargo build --bins
cd ../tests/ui && npm install && npx playwright install chromium
npm test
```

`chat.spec.mjs` sends a prompt to whichever model `.env` wires to the current
slot, so that spec needs a working `SYN_API_KEY` and costs a call. The other
two specs are offline.
