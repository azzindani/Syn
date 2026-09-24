# Capability test v2: the executive review

v1 asked for a summary table, one pivot, two charts and a page of prose. A
model produced two empty sheets and no memo, and a scripted control showed
the harness could express all of it. That is a floor, not a product.

v2 is the job an analyst is actually paid for: a workbook a stakeholder can
drive themselves, and a report a director can take into a meeting. It is
roughly ten times the work, and it needs capabilities the engine does not
have yet. The gap list at the bottom is the build order.

Passing this is the bar for calling the engine **v0.1.0**.

## The fixture

Unchanged from v1: `samples/Solar_Energy_Production.csv`, 258,423 hourly kWh
readings, 11 City of Calgary sites, 2015-2023, loaded into live Excel as
`testbed/docs/solar.xlsx` sheet `data`. A Word document is open beside it.

Ground truth is computed from the CSV by `ground-truth.json` and never from
Excel, so a wrong number in the workbook cannot also be the yardstick.

    11 sites          9,835,517.9 kWh        91 year-months
    top site          Bearspaw Water Treatment Plant   3,082,637.6 kWh
    best year         2020, 1,765,072 kWh    worst full year 2015 (partial)
    seasons           Summer 4,040,022  Spring 3,331,244  Autumn 1,856,771  Winter 607,481
    peak hour         13:00, 1,300,197 kWh
    peak / trough     July 1,444,872  vs  December 93,541   (15x)
    top site median   61.664 kWh      p95 420.039      peak 514.462

## The brief

One message, in the ordinary chat UI. It names no tool, no selector, no cell
address and no handle.

> You have solar.xlsx open with 258,423 hourly kWh readings from 11 City of
> Calgary solar sites on sheet 'data' (columns A-H: name, id, address, date,
> kWh, public_url, installationDate, uid), and a Word document open beside
> it. Build the quarterly performance review the energy team takes to the
> city's infrastructure committee.
>
> In the workbook I want a model I can interrogate, not a screenshot: the raw
> data as a proper Excel table with derived year, month, hour and season
> columns; a per-site scorecard carrying total, mean, median, 95th
> percentile, peak, reading count and share of estate output, every one of
> them a live formula; monthly and seasonal breakdowns; an hour-of-day
> generation profile; and a ranking sheet that shades good and bad
> performance so it reads at a glance. Give me a dashboard sheet that a
> committee member can actually use - several charts laid out deliberately,
> titled, with a slicer so they can filter by site themselves.
>
> Then write the report in Word. Twenty pages, structured: title page,
> contents, an executive summary that leads with the finding rather than the
> method, then sections covering estate performance, site-by-site results,
> seasonal and daily patterns, data quality, and recommendations. Use proper
> headings, tables, and the charts from the workbook. Every number in the
> prose must match the workbook. Tell me what the data means and what you
> would do about it, not what you did.

## Scoring

Machine-read by `score-v2.ps1` from the live documents and the saved
transcript. Nothing is graded from the model's own account of itself.

### Excel: the model a stakeholder can drive (30 points)

| # | Check | Pts |
|---|---|---|
| X1 | The raw data is a real Excel Table (ListObject), not a bare range | 2 |
| X2 | Derived columns exist and are formulas: year, month, hour, season | 3 |
| X3 | A per-site scorecard names all 11 sites | 2 |
| X4 | It carries >= 6 metric columns, all `HasFormula` | 4 |
| X5 | Total, mean, median, p95, peak and count each within 1% of truth for every site | 5 |
| X6 | Share-of-estate column sums to 100% (+/- 0.5) | 1 |
| X7 | A monthly breakdown with >= 91 year-month rows or an equivalent pivot | 2 |
| X8 | A seasonal breakdown, four seasons, each within 1% of truth | 2 |
| X9 | An hour-of-day profile, >= 12 hour rows, peak hour matches truth | 2 |
| X10 | >= 2 PivotTables in the workbook | 2 |
| X11 | >= 1 slicer, connected to a pivot | 2 |
| X12 | Conditional formatting present: >= 2 rules of >= 2 distinct kinds | 2 |
| X13 | >= 1 named range | 1 |

### Excel: the dashboard (10 points)

| # | Check | Pts |
|---|---|---|
| D1 | A dashboard sheet exists and is not the data sheet | 1 |
| D2 | It carries >= 5 chart objects | 3 |
| D3 | >= 3 distinct chart types among them | 2 |
| D4 | Every chart has a title that is not the Excel default | 2 |
| D5 | Charts are laid out, not stacked: no two overlap | 2 |

### Word: the report (20 points)

| # | Check | Pts |
|---|---|---|
| W1 | >= 20 pages | 4 |
| W2 | >= 8 paragraphs styled as a real Heading level | 3 |
| W3 | A table of contents field | 2 |
| W4 | >= 3 Word tables with >= 3 rows each | 3 |
| W5 | >= 4 inline shapes or pictures (the charts) | 3 |
| W6 | A header or footer carrying a page number field | 2 |
| W7 | >= 2,500 words of body text | 3 |

### Judgement: is it worth reading (10 points)

| # | Check | Pts |
|---|---|---|
| J1 | Names the correct top site and its share of estate output | 2 |
| J2 | States the seasonal swing with the right direction and magnitude | 2 |
| J3 | Identifies the peak generating hour correctly | 2 |
| J4 | Flags the partial years (2015, 2023) as a data-quality caveat | 2 |
| J5 | Every kWh figure in the prose matches the workbook to within 1% | 2 |

### Process (10 points)

| # | Check | Pts |
|---|---|---|
| P1 | Every tool call issued was executed | 2 |
| P2 | >= 90% executed rather than refused | 2 |
| P3 | The doom-loop gate never fired | 1 |
| P4 | No `shell` attempt | 1 |
| P5 | Ended with a prose answer, not a stop | 2 |
| P6 | Finished inside the step budget | 2 |

**80 points total.**

- **68-80** — v0.1.0. A stakeholder could use this without knowing a model made it.
- **52-67** — the engine is sound, the output needs an editor.
- **30-51** — it builds parts of a review, not a review.
- **0-29** — not yet an engine.

## Honesty conditions

Void, and says so, if: the scorer reads the model's prose instead of the
documents; the brief names a cell, selector, handle or tool; a check is
added, removed or loosened after seeing a run; or setup writes any part of
the expected output.

Committed before the tools it needs exist. Git order is the evidence.

## What the engine cannot do yet

The gap between v1 and this, and therefore the build order. Struck rows are
closed and verified against live Office; the note says how.

**Excel**

| Need | State |
|---|---|
| ~~One formula filled down 258,423 rows~~ | closed: a one-cell payload over a multi-cell range fills it. 1.03M formula cells in 11s |
| ~~Excel Table (ListObject)~~ | closed: `table` |
| ~~Conditional formatting~~ | closed: `conditional`, five rule kinds |
| ~~Slicer~~ | closed: `slicer` |
| ~~Named range~~ | closed: `name` |
| ~~Chart size and position~~ | closed: anchor a chart to a *range* and it fills exactly those cells. Anchoring to a single cell left Excel's 440x260 default, and five charts came out with eight overlapping pairs |
| ~~Chart axis titles, legend, gridlines~~ | closed: `style` on `chart`, same `k=v;k=v` shape `format` uses |
| ~~Fill and font colour~~ | closed: the Rust side always allowed `fill`/`color`/`font`; the sidecar quietly did not |
| ~~Freeze panes, sheet-wide autofit~~ | closed: `freeze`, `autofitSheet` on `format` |
| Pivot grouped by month/year from a date column | open. Derived Year/Month columns are the workaround, and are faster over 258k rows |
| Pivot with several value fields | open. One value field per pivot |

**Word**

| Need | State |
|---|---|
| ~~Append a paragraph~~ | closed: `insertParagraph`. Assigning `Range.Text` ate the paragraph mark and merged the new paragraph into the next, taking its style with it: one heading survived out of five. It appends with `InsertAfter` now |
| ~~Heading and body styles~~ | closed: the style rides in `name`, and goes on the Paragraph rather than its Range |
| ~~Tables~~ | closed: `insertTable`, same pipe/semicolon grid the sheet uses |
| ~~Page break, section break~~ | closed: `pageBreak` |
| ~~Table of contents field~~ | closed: `contents`. A contents page is written before the sections it lists, so calling it again refreshes the one already there rather than stacking a second |
| ~~Header/footer with page numbers~~ | closed: `pageNumbers`. `Fields.Add` at a collapsed range pushes what was there to the right, so PAGE then NUMPAGES produced "Page  of 41"; the fields are placed at explicit offsets, later slot first |
| ~~Pictures, and pasting an Excel chart~~ | closed: `export png` writes every chart in the workbook to disk, `picture` places one |

Everything above is ordinary late-bound COM. The engine had simply never
been asked for it.

## Results

| Run | Score | What it says |
|---|---|---|
| 2026-09-19, control (`load.txt`, scripted) | **70/70** artifacts and judgement | Every part of the job is expressible and lands correctly. 180 operations, 18s. |
| provider run | not yet run | |

The control is run with `score-v2.ps1 -Control` and is deliberately scored
out of 70: it has no transcript, so the ten process points do not apply, and
totalling them from some other run would report a number nobody earned. It
is **not a pass**. It isolates one variable -- whether the harness can
express the job at all -- from the other -- whether a model can drive it.

Two scorer defects were fixed after the first control run and before any
model run, and both were false negatives rather than loosened checks: J2
did not recognise "a factor of 6.7", only "6.7x"; and J5 held no reading
counts in its truth set, so a correct figure in the prose scored as a miss.
Git order is the evidence.

## Engine load test

Separate from the capability test, and not scored: it answers "can the
engine carry this volume", not "can a model plan it". `load.txt` is scripted
by hand, so it says nothing about the model.

    180 operations, 18s, 0 failures
    1,033,692 derived formula cells over 258,423 rows
    11-site scorecard, 7 metrics each, every one a live formula
    108 year-month rows, 24 hour rows, 4 seasons
    5 charts laid out by range anchor, 2 pivots, 1 slicer, 6 shading rules
    a 20-page report: contents, 24 headings, 4 tables, 5 charts, 3,286 words

Every figure exact against `ground-truth.json`: all 11 site totals, the four
season totals, peak hour 13:00 at 1,300,197 kWh, and 91 non-zero year-months
summing to 9,835,517.9 kWh.

The run found four defects that the per-verb smoke tests had not: charts
overlapping at cell anchors, the sidecar dying at startup when Excel was
momentarily busy, `fill`/`color` accepted by Rust and refused by the
sidecar, and the CLI reading a multi-word chart title into the style slot.
