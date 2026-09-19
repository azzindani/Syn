# Capability test v3: the executive review, and the room it is presented in

v2 asked for a workbook a stakeholder can drive and a report a director can
take into a meeting. v3 adds the third thing that job actually produces: the
deck the analyst stands up and presents from.

Three applications, one model, one brief, no plugins. That is the whole
claim, and this is the test of it.

Passing this is the bar for calling the engine **v0.1.0**.

## The fixture

`samples/Solar_Energy_Production.csv`, 258,423 hourly kWh readings, 11 City
of Calgary sites, 2015-2023, loaded into live Excel as
`testbed/docs/solar.xlsx` sheet `data`. A Word document and an empty
PowerPoint deck are open beside it.

The deck starts with **zero slides**. Unlike the memo, which ships with
blank paragraphs to write into, there is nothing to fill: every slide has to
be built.

Ground truth is computed from the CSV by `ground-truth.json` and never from
Excel, so a wrong number in the workbook cannot also be the yardstick.

    11 sites          9,835,517.9 kWh        91 year-months
    top site          Bearspaw Water Treatment Plant   3,082,637.6 kWh
    best year         2020, 1,765,072 kWh    partial years 2015 and 2023
    seasons           Summer 4,040,022  Spring 3,331,244  Autumn 1,856,771  Winter 607,481
    peak hour         13:00, 1,300,197 kWh
    peak / trough     July 1,444,872  vs  December 93,541   (15x)
    top site median   61.664 kWh      p95 420.039      peak 514.462

## The brief

One message, in the ordinary chat UI. It names no tool, no selector, no cell
address and no handle.

> You have solar.xlsx open with 258,423 hourly kWh readings from 11 City of
> Calgary solar sites on sheet 'data' (columns A-H: name, id, address, date,
> kWh, public_url, installationDate, uid), a Word document open beside it,
> and an empty PowerPoint deck. Build the quarterly performance review the
> energy team takes to the city's infrastructure committee.
>
> In the workbook I want a model I can interrogate, not a screenshot: the raw
> data as a proper Excel table with derived year, month, hour and season
> columns; a per-site scorecard carrying total, mean, median, 95th
> percentile, peak, reading count and share of estate output, every one of
> them a live formula; monthly and seasonal breakdowns; an hour-of-day
> generation profile; and a ranking that shades good and bad performance so
> it reads at a glance. Give me a dashboard sheet a committee member can
> actually use - several charts laid out deliberately, titled, with a slicer
> so they can filter by site themselves.
>
> Then write the report in Word. Twenty pages, structured: title page,
> contents, an executive summary that leads with the finding rather than the
> method, then sections covering estate performance, site-by-site results,
> seasonal and daily patterns, data quality, and recommendations. Use proper
> headings, tables, and the charts from the workbook.
>
> Then build the deck I present from. It is fifteen minutes in front of a
> committee, not a document on a screen: a title slide, section dividers, a
> handful of slides that each make one point, the charts rather than tables
> of numbers wherever a chart will do, and speaker notes telling me what to
> say on each one. Lead with the finding.
>
> Every number in the prose and on the slides must match the workbook. Tell
> me what the data means and what you would do about it, not what you did.

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

### PowerPoint: the deck (20 points)

| # | Check | Pts |
|---|---|---|
| K1 | A deck of >= 12 slides | 3 |
| K2 | A title slide and >= 2 section dividers | 2 |
| K3 | >= 4 distinct slide layouts used | 2 |
| K4 | >= 4 charts placed on slides as pictures | 3 |
| K5 | >= 1 table of >= 3 rows | 2 |
| K6 | >= 6 slides carry speaker notes | 3 |
| K7 | Slide numbers are on | 1 |
| K8 | Every slide has a title that is not empty | 2 |
| K9 | Every kWh figure on the slides matches the data to within 1% | 2 |

### Process (10 points)

| # | Check | Pts |
|---|---|---|
| P1 | Every tool call issued was executed | 2 |
| P2 | >= 90% executed rather than refused | 2 |
| P3 | The doom-loop gate never fired | 1 |
| P4 | No `shell` attempt | 1 |
| P5 | Ended with a prose answer, not a stop | 2 |
| P6 | Finished inside the step budget | 2 |

**100 points total.**

- **85-100** — v0.1.0. A stakeholder could use all three without knowing a
  model made them.
- **65-84** — the engine is sound, the output needs an editor.
- **38-64** — it builds parts of a review, not a review.
- **0-37** — not yet an engine.

## Honesty conditions

Void, and says so, if: the scorer reads the model's prose instead of the
documents; the brief names a cell, selector, handle or tool; a check is
added, removed or loosened after seeing a run; or setup writes any part of
the expected output.

v3 adds the deck to v2. It is a new rubric rather than an edit to the old
one, because v2 already carries a recorded result and moving the bar under
a published number is exactly what the conditions above forbid. No model
run has been scored against either.

Three scorer defects were fixed during the control runs and before any
model run. All three were false negatives rather than loosened checks, and
git order is the evidence:

- J2 recognised "6.7x" but not "a factor of 6.7", which is how anyone
  actually writes it.
- J5 held no reading counts in its truth set, so a correct figure scored as
  a miss.
- J5 and K9 counted "a factor of 2,310" as a kWh figure and looked for it
  in the data. It is a ratio. Both checks are for kWh figures, which is
  what J5 always said and what K9 now says too.

Two more surfaced in the first model run, and this is where the rule above
bites hardest, because the party who would benefit from bending it is the
one who wrote both the engine and the scorer:

- The scorecard was found by taking the first sheet with site names down a
  column. The run abandoned one attempt on `Scorecard` and built the real
  thing on `Scorecard2`, so X4, X5 and X6 were graded against the
  wreckage. It now takes the candidate carrying the most numbers.
- X8 looked for the literal season names in `ground-truth.json`. Excel's
  own grouping says "Fall" where the fixture says "Autumn", so four
  correct totals scored zero.

Two runs of the same model on the same task scored 19 and 10. Neither
finished Excel, neither opened Word or PowerPoint, and both ended on an
empty completion. A single run is not a measurement of a model; what these
two establish together is the size of the gap, not its exact width.

**The 19/100 above stands.** It was not re-scored under the repaired
scorer, and it must not be: re-grading a recorded run after changing the
grader is the thing the honesty conditions exist to prevent. The fixes
apply to the next run, which starts from a clean fixture.

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

**PowerPoint**

| Need | State |
|---|---|
| ~~A slide with content on it~~ | closed: `createSlide` takes a title, bullets and a layout. It used to add a slide with a title and nothing else |
| ~~Layouts~~ | closed: title, titleContent, sectionHeader, twoContent, comparison, titleOnly, blank |
| ~~Sub-bullets~~ | closed: a leading `>` per level. Leading spaces were the obvious marker and do not survive the grid splitter, which trims every cell |
| ~~Pictures on a slide~~ | closed: `picture` with a slide selector and a left,top,width,height box |
| ~~Tables on a slide~~ | closed: `insertTable` with a slide selector |
| ~~Speaker notes~~ | closed: `write` to `s3.notes` |
| ~~Slide numbers and a footer~~ | closed: `pageNumbers` |
| ~~Finding the right deck~~ | closed: by name, like the workbook and the document. It used to drive `ActivePresentation`, which is whatever the human last clicked on |

Everything above is ordinary late-bound COM. The engine had simply never
been asked for it.

PowerPoint's object model differs from the other two in one way that cost
three rounds of debugging: `Visible`, `DisplayAlerts`, `HasTextFrame` and
`HasTable` are tri-states and enums where Word and Excel use booleans, so
the boolean form fails the cast rather than the call.

## Results

| Run | Score | What it says |
|---|---|---|
| 2026-09-19, control (`load.txt`, scripted) | **90/90** artifacts and judgement | Every part of the job is expressible and lands correctly across all three applications. 217 operations, 37s. |
| 2026-09-19, model-driven, `nemotron-3-ultra-550b:free` | **19/100** | Real Excel Table, four derived columns, a scorecard naming all 11 sites, four pivots, three named ranges. Then it ended the turn with no answer and no tool call, 45 calls in and well inside the budget. Word: 5 words. Deck: 0 slides. |
| 2026-09-19, same model, repeat | **10/100** | Same brief, same fixture, repaired scorer, empty-turn nudge in place. Got nine calls in: the Table, and derived headers written one cell at a time. The nudge fired and it did carry on, then ended the same way. Word: 5 words. Deck: 0 slides. |
| provider run | not yet run | |

The control is run with `score-v3.ps1 -Control` and is deliberately scored
out of 90: it has no transcript, so the ten process points do not apply, and
totalling them from some other run would report a number nobody earned. It
is **not a pass**. It isolates one variable -- whether the harness can
express the job at all -- from the other -- whether a model can drive it.

## Engine load test

Separate from the capability test, and not scored: it answers "can the
engine carry this volume", not "can a model plan it". `load.txt` is scripted
by hand, so it says nothing about the model.

    217 operations, 37s, 0 failures, three applications
    1,033,909 derived formula cells over 258,423 rows
    11-site scorecard, 7 metrics each, every one a live formula
    108 year-month rows, 24 hour rows, 4 seasons
    5 charts laid out by range anchor, 2 pivots, 1 slicer, 6 shading rules
    a 20-page report: contents, 24 headings, 6 tables, 5 figures, 3,473 words
    a 14-slide deck: 4 layouts, 3 dividers, 4 charts, notes on every slide

Every figure exact against `ground-truth.json`: all 11 site totals, the four
season totals, peak hour 13:00 at 1,300,197 kWh, and 91 non-zero year-months
summing to 9,835,517.9 kWh.

Building it at full scale is what finds the defects. The Excel pass found
four the per-verb smoke tests had not; the PowerPoint pass found four more,
three of them the same mistake in different places -- `Visible`,
`DisplayAlerts`, `HasTextFrame` and `HasTable` are tri-states and enums
where Word and Excel use booleans, so the boolean form fails the cast
rather than the call, and one of those failures was being swallowed by a
`catch` and read as "this slide has no text".
