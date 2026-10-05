# v0.1.0 release checklist (Windows, Office)

What has to be proven on a Windows machine with desktop Office before
tagging `v0.1.0`. Everything else is proven by CI (Rust tests on three
platforms, both C# helpers compile, every PowerShell script parses, the
LibreOffice live suite, the package builds). These are the things only
real Office, a real window and a real double-click can show.

Work on the throwaway documents in `testbed\`, never on your own. Tick each
box; a failure is a bug to fix before the tag, not a note.

## 1. The installer, as a new user gets it

Download the `syn-windows` artifact of the latest green CI run (Actions →
the run → Artifacts), or build it: `powershell -File scripts\package.ps1`.

- [ ] Run `Syn-<version>-setup.exe` on a machine (or user account) with
      **no Rust, no .NET SDK and no `.env`** from the repository. No
      administrator prompt; SmartScreen warns about an unsigned program
      (More info → Run anyway) -- note that it did.
- [ ] "Start Syn now" on the last page: one Syn window, and **no terminal
      window** on the taskbar. Next time, the Start menu or desktop "Syn".
- [ ] The key icon in the sidebar opens Settings; paste a key; the "Add an
      API key" notice goes away.
- [ ] Ask: `open <full path>\testbed\docs\plan.xlsx`. Excel opens it (or
      takes it up if already open), the reply says it is open, and **it does
      not go on to read or change anything**.
- [ ] Ask for a small change ("put a header row in bold with a fill on
      Sheet1"). One or two calls, not one per cell.
- [ ] Close the Syn window. In Task Manager: no `ui.exe`, `cli.exe`,
      `office-host.exe` or `curl.exe` left. Excel is still open with the
      document.
- [ ] `.agent\` appeared beside `.env` in `%LOCALAPPDATA%\Programs\Syn`
      (chats, the saved key), not somewhere else.
- [ ] Run the installer again over it: the chats and the key survive.
- [ ] Uninstall from Settings > Apps, answering No to deleting chats: the
      programs and shortcuts go, `.agent\` and `.env` stay. Uninstall again
      after reinstalling, answering Yes: the folder is gone.
- [ ] Start Syn from the Start menu: it opens in a window of its own titled
      Syn (no tabs, no address bar, no terminal window), with the Syn icon
      on the taskbar, whatever the default browser is. Start it again from the desktop icon: the same
      window comes to the front, and no second one opens.
- [ ] Minimise the window and start Syn again: it comes back.
- [ ] Move and resize the window, close it, start Syn again: it opens where
      it was.
- [ ] A link in a reply opens in your own browser, not in the Syn window.
- [ ] On a machine with no WebView2 runtime (an older Windows 10): Syn still
      opens, as an Edge application window.
- [ ] The zip: unzip into `Documents\Syn-test`, double-click `syn.exe`,
      and it works the same.

## 2. Every Office verb, through the real helper

```
powershell -File scripts\new-testbed-docs.ps1     # close Office first
powershell -File scripts\live-office-peak.ps1     # one PASS/FAIL line per step
```

- [ ] `live-office-peak.ps1` exits 0. New in this round, all in it:
      the Excel table with a design (`TableStyleMedium9`), its undo, and a
      Word table style refused for Excel.
- [ ] Run it a second time: it passes again (it clears its own old
      exports; an export never replaces a file the session did not write).

## 3. What this round changed and only Windows can show

- [ ] **Same name, other folder.** Open `testbed\docs\plan.xlsx` in Excel
      yourself. Copy it to `testbed\other\plan.xlsx` and ask Syn to open
      that one. It must refuse ("a different plan.xlsx is already open …"),
      not work on your copy. Repeat with a `.docx` in Word.
- [ ] **Nothing left behind.** With the console running and a turn going
      (ask for something long), end `ui.exe` in Task Manager. Within a few
      seconds `cli.exe`, `office-host.exe` and `curl.exe` are gone too
      (the job object). Office and its documents stay.
- [ ] **Stop button.** During a long turn, press Stop. The run ends; if it
      had to restart the CLI, the page reloads onto the same chat.
- [ ] **Exports stay home.** Pick a workspace folder above the message box,
      then ask to "export the workbook to C:\Windows\Temp\x.xlsx". Refused:
      outside the workspace. Ask to export over a file already in the
      workspace that Syn did not write: refused.
- [ ] **Non-Latin text.** Put a long Indonesian or Chinese paragraph, with
      em dashes, in a Word document and ask Syn to read it. No crash.
- [ ] **A CSV.** Ask Syn to turn a `.csv` into an `.xlsx`. It should be
      `open` then `export`: two calls.

## 3b. The browser, on a real desktop

```
powershell -File scripts\live-cdp-smoke.ps1     # Chrome, window showing
powershell -File scripts\live-cdp-smoke.ps1 -Browser edge
```

- [ ] Both suites pass in Chrome and in Edge, and the script ends with
      "clean: no test browser processes remain".
- [ ] On a machine with only Edge (no Chrome), ask Syn in the console to
      open a page and read its heading: a browser window of its own
      opens, titled by the page. It is not your everyday profile (none of
      your sign-ins are in it).
- [ ] Ask Syn to sign in to a site: it fills the name, refuses the password
      field and asks you to type it. Type it in the window, say so, and it
      carries on. Close Syn, start it again, ask for the same site: still
      signed in.
- [ ] Close the browser window while Syn is open, then ask for a page: it
      says the window was closed and opens one again.
- [ ] Close Syn: no `chrome.exe` or `msedge.exe` of Syn's own is left
      (Task Manager, Details, command line has `browser-profile`). Your own
      Chrome windows are untouched.
- [ ] With `AGENT_BROWSER=off`, the model is told it has no browser rather
      than failing at a call.

## 4. From an MCP client

- [ ] Point Claude Desktop (or another client) at the package's
      `mcpgate.exe`. `status` lists Excel, Word and PowerPoint as
      available with no `.env` changes.
- [ ] Open a workbook and read a range through the client.
- [ ] Quit the client: `mcpgate.exe` and `office-host.exe` go with it.

## 5. The dev checks, on Windows

```
cd core && cargo build --bins && cd ..
python tests\dev\turn.py
python tests\dev\leftovers.py
cd tests\ui && npm install && npm test && npm run audit
```

- [ ] `turn.py` all PASS (its Windows process listing has not run before).
- [ ] `leftovers.py` all PASS, including the browser scenario (a forced
      kill of Syn with its browser open leaves no browser process); the
      Office MCP scenario skips on Windows, which §4 covers by hand.
- [ ] The UI specs pass except `chat.spec.mjs` without a key; `npm run
      audit` all PASS.

## 6. Tag

- [ ] Every box above ticked, CI green on the commit.
- [ ] `git tag v0.1.0 && git push origin v0.1.0`. CI builds the package
      again and publishes it as the `v0.1.0` release with generated notes.
