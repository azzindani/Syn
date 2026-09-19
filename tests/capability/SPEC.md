# Capability test: the analyst job

One harness, many hands, no plugins. This is the test that says whether that
claim survives contact with a real job rather than a toy one.

It is deliberately a job nobody would call easy: a quarter of a million rows
of real municipal data, four Excel capabilities that each need a different
part of the COM surface, and a written deliverable in a second application.
A pass means Syn drove two applications it did not write, through one model,
to an output a person would accept.

## The fixture

`samples/Solar_Energy_Production.csv` — 258,423 hourly kWh readings from 11
City of Calgary solar installations, 2015-2023. Loaded into live Excel as
`testbed/docs/solar.xlsx`, sheet `data`, columns A-H:

    name, id, address, date, kWh, public_url, installationDate, uid

A Word memo `testbed/docs/solar-memo.docx` is open alongside it.

Ground truth is computed straight from the CSV by `ground-truth.json`, never
from Excel, so a wrong answer in the workbook cannot also be the yardstick.

    11 sites, 9,835,517.9 kWh total, 91 distinct year-months
    top site: Bearspaw Water Treatment Plant, 3,082,637.6 kWh
    seasonal swing: July 1,444,872 kWh vs December 93,541 kWh (15x)

## The brief

Given to the model verbatim, as one message, in the ordinary chat UI:

> solar.xlsx is open: 258,423 hourly kWh readings from 11 City of Calgary
> sites on sheet 'data', columns A-H = name, id, address, date, kWh,
> public_url, installationDate, uid, with a header row. Do the work an energy
> analyst would, end to end. On a Summary sheet build a per-site table with
> total kWh, mean hourly kWh, peak hourly kWh and reading count, as live Excel
> formulas that recalculate rather than pasted numbers. Pivot total kWh by
> month against site. Build a Dashboard sheet with a line chart of monthly
> total output and a bar chart of the top ten sites. Bold the header rows, put
> thousands separators on the kWh columns and widen the site name column. Then
> write your findings into the open Word memo: which sites dominate, how output
> swings seasonally, and anything you would flag to the facilities team.

Nothing in the brief names a tool, a selector, a cell address or a handle.
That is the point: a person asks for the job, not for the keystrokes.

## Scoring

Every check is objective and machine-read by `score.ps1` against the live
documents and the saved transcript. No check is graded by reading the
model's own account of what it did — several earlier runs reported work
that had not happened.

### Output: did the artifacts land (10 points)

| # | Check | Points |
|---|---|---|
| O1 | A `Summary` sheet exists | 1 |
| O2 | It names all 11 sites | 1 |
| O3 | Its totals are live formulas (`HasFormula`), not pasted values | 2 |
| O4 | Every per-site total is within 1% of ground truth | 2 |
| O5 | A monthly view exists: a real PivotTable, or a grid with >= 12 month rows | 1 |
| O6 | A `Dashboard` sheet carries >= 2 chart objects | 1 |
| O7 | Header row bold, and a kWh column formatted with a thousands separator | 1 |
| O8 | The memo has >= 5 non-empty paragraphs beyond its title | 1 |

### Judgement: is the writing worth reading (4 points)

| # | Check | Points |
|---|---|---|
| J1 | The memo names the correct top site | 2 |
| J2 | The memo states the seasonal pattern with the right direction | 2 |

### Process: did it get there sanely (6 points)

Read from the saved chat transcript, not from the model's summary.

| # | Check | Points |
|---|---|---|
| P1 | Every tool call issued was executed (no silent drops) | 1 |
| P2 | >= 90% of calls executed rather than declined | 1 |
| P3 | The doom-loop gate never fired | 1 |
| P4 | No `shell` attempt: it stayed inside the document ops | 1 |
| P5 | Ended with a prose answer, not a stop or an empty turn | 1 |
| P6 | Finished inside the step budget | 1 |

**20 points total.**

- **17-20** — the concept holds. A person could have asked for this and walked away.
- **12-16** — the harness works, the output needs a human pass.
- **6-11** — parts drive, the job does not complete.
- **0-5** — the concept does not survive a real task.

## Honesty conditions

The test is void, and says so, if any of these hold:

- the scorer reads anything from the model's prose rather than the documents
- the brief names a cell, a selector, a handle or a tool
- a check is added, removed or loosened after seeing a run
- setup writes any part of the expected output

The rubric above was committed before the supporting ops were written. Its
git history is the evidence.

## Results

| Run | Score | What it says |
|---|---|---|
| 2026-09-19, model-driven, `ling-3.0-flash-vl:free` | **6/20** | Summary sheet, Dashboard sheet and a real PivotTable created; no table filled, no charts, memo untouched. Ended mid-job with no answer and no tool call. |
| 2026-09-19, control (`control.txt`, scripted) | **14/14** artifacts and judgement | Every required operation is expressible and lands correctly. Per-site totals exact against ground truth. |

The control is run with `score.ps1 -Control` and is deliberately scored out
of 14: it has no transcript, so the six process checks do not apply to it,
and totalling them from the last model run would report a number nobody
earned. It is not a pass. It isolates one variable -- whether the harness can
express the job at all -- from the other -- whether the model can drive it.

On this evidence the binding constraint is the model, not the harness.
