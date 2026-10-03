# What a natural session must look like to count as passing

Written on 2026-10-03, after four 50-message runs (hotels, oil, logistics
twice) had shown what goes wrong, and **before** the runs that are scored
against it. The goal behind it: a person talking to Syn in plain sentences
should get the work done by the harness and the MCP server **the way they
would expect a colleague to do it** -- quickly, correctly, without anyone
having to rescue it, and without leaving a mess.

These are fixed. A criterion is never loosened, dropped or reworded after a
run has been seen; a change goes in as a new version of this file with the
reason, and runs made under the old one stay scored under the old one. The
prompts of a script are fixed the same way (`README.md`).

A run is **one script played in order against a fresh workspace** with the
model the deployment uses (`AGENT_MODEL_*`), driven by `drive.ps1` and scored
by `score.py`. Gates A, C, D and the count parts of E are scored by machine;
B, F and G by reading the run against the checks in the script's header.

## Whose fault

Every failure is put in one of three boxes, and only the first two block:

- **Harness**: the loop, the tool schemas, the manual, the retry and recovery
  rules, the console. Anything the model could not reasonably have avoided
  or was told wrongly.
- **MCP / Office helper**: `mcpgate`, `office-host`, `sidecar-lo`. A call
  that corrupts, hangs, flashes, leaves something behind or reports wrongly.
- **Model**: a wrong conclusion or a wasteful route the tools did allow
  and the manual described. Not a blocker, but counted (gate B and C), and a
  *pattern* of it is a harness problem: it means the guidance is not doing its
  job.

## The gates

### A. The session finishes by itself

- **A1.** Every one of the 50 turns ends with an `ANSWER`. None ends `STOPPED`
  or `ERROR` for a reason on the harness side. A provider outage that the retry
  rules cannot outlast is listed, not counted.
- **A2.** Nobody intervenes: no process killed, no box clicked, no restart, no
  retyped prompt. (The scorer flags any turn over 20 minutes as presumed
  rescued.)
- **A3.** No turn runs out of its step budget (100).
- **A4.** The source files are byte-for-byte what they were, and no file the
  user did not ask for is written outside the workspace.

### B. The answers are right

- **B1.** Every fact in the script's header ("what a careful analyst finds")
  that a prompt asks for is answered exactly: figures to the digit, counts
  exactly, a named top-N in the right order.
- **B2.** Every trap in the header is either caught or, at worst, not asserted
  wrongly. A confident wrong answer to a checkable question counts against
  the model; **one caused by the tools** (a blank turned into 0, a date turned
  into a serial number, a count of the wrong range, a result read from the
  wrong sheet) blocks, whatever the model did.
- **B3.** What the model says it did is what the document holds. "Saved",
  "removed", "added a chart", "sheet is hidden" are checked against the file
  or the application after the run.

### C. It is quick

- **C1.** Median turn at most 60 s; 90th percentile at most 240 s.
- **C2.** At most 3 turns use 60 steps or more, and none uses 100.
- **C3.** The whole script finishes in at most 90 minutes with no hang.

### D. It behaves like a careful person

- **D1.** It does what was asked and stops. At the end the workbook holds the
  sheets the script asked for plus at most two helper sheets, no stray cells
  to the right or below the data, no `test`, `scratch` or `work` sheets, no
  experiments in a source file.
- **D2.** At most 8% of tool calls are refused, and none is refused for a
  reason the tool's own description or the manual should have prevented. A
  refusal's text names the fix.
- **D3.** The repeated-call gate ("this is the same call as the two before")
  never fires.
- **D4.** It does not touch what is not Syn's: the user's own workbooks and
  documents, other applications, the clipboard.

### E. It shows its work well

- **E1.** Every list and table in an answer is drawn as a list and a table in
  the console (the page specs hold this).
- **E2.** No blank Office window is visible for more than 500 ms (the window
  sampler, `testbed/win-sampler-all.ps1`).
- **E3.** The console never shows a turn as running after it ended, and never
  shows an answer twice.

### F. It recovers without help

- **F1.** A call that does not come back is dealt with by the helper inside
  `AGENT_OFFICE_CALL_SECS` plus its grace, the model is told in words what
  happened and what to do instead, and the session carries on.
- **F2.** A box on the screen is reported with its text; the one safe box
  (links) is answered. Nothing is killed, and no button but that one is pressed.
- **F3.** A dropped connection or a rate limit is waited out and the turn goes
  on.

### G. The documents come out right

- **G1.** The Word document and the deck the script asks for exist, are saved
  where the script says, and have the structure the prompts asked for (title,
  one section per prompt, header and footer, slide count after the edits).
- **G2.** Numbers in them are the numbers from the workbook.
- **G3.** Tables and charts are in them, not only described.

## When it is done

All of these hold:

1. **Two consecutive scored runs on two different datasets** pass A, C, D, E
   and F in full, and B and G with no harness-caused failure and at most one
   model-caused wrong answer per run.
2. **No open harness or MCP defect** from either run is left unfixed or
   undocumented as a known limit in `docs/`.
3. Everything fixed along the way has a test (Rust test, page spec or peak
   step) and a live check on Office, named in its commit message.
4. `cargo clippy --all-targets -- -D warnings`, `cargo test`, the page specs
   and the Excel section of `scripts/live-office-peak.ps1` pass on the final
   commit, and CI is green.

A run that cannot finish for a reason outside Syn (the provider down for
hours, the machine asleep) is void, stays in the record marked void, and is
played again.

## What a fix must not do

- Name a dataset, a file, a column or a figure in guidance, schemas or coach
  rules (`core/tests/no_dataset_in_guidance.rs` holds this).
- Make Syn touch a document it was not asked to, or kill an application.
- Make the model's work less visible or a refusal less clear to pass a gate.
- Reword a prompt, or edit a script's header after seeing a run.
