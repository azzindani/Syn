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

> **All four results below are void.** The runs did not fail the way the
> notes say. An upstream outage — `{"error":{"message":"Upstream error
> from Nvidia: Service temporarily overloaded","code":503}}` — arrives
> from OpenRouter with **HTTP 200**, and the engine only treated a
> non-200 as a failure. The error body reached the agent, which found no
> content and no tool calls in it and reported that the model had ended
> the turn with nothing to say. Three of the four "empty completions"
> were that. The models were not giving up; they were never asked.
>
> Fixed, with a test, and every run below must be repeated before any of
> it means anything. Kept here rather than deleted, because a rubric that
> quietly loses its wrong answers cannot be checked.

Four runs, three models, 8 to 19 out of 100 against a control that scores
90/90 on the same rubric and the same fixture. None finished Excel. None
opened PowerPoint. Three of the four managed a single operation.

Between them they found two ways for a run to end early, and neither is a
missing capability:

- three times, an empty completion with the budget barely touched;
- once, prose narrating the plan before any work -- "I'll start by
  exploring the dataset" -- which the loop accepted as the finished job.

Both are now patched, and run 4 was the first with both in place. It was
nudged after its empty turn and returned a second empty turn. The nudges
buy steps from a model that has a plan and lost its place; they cannot
supply a plan to one that never had one.

Every model here is on a free tier. That is the condition the runs were
made under and it bounds what they can conclude: they establish that these
four free models cannot hold a job of this size, and they say nothing
about models in general.

A floor worth knowing when reading the table: the six process checks are
worth ten points and mostly pass for any run that does not crash, so 8/100
is roughly what a run scores for making one tool call and stopping. The
discriminating range is 8 to 100, not 0 to 100. Noted for v4 alongside the
P5 defect.

A note on P5, which run 3 passed. It scores "ended with a prose answer,
not a stop", and a run that narrates its intention and quits earns those
two points while doing nothing. That is a defect in the check. It is
recorded here rather than repaired, because tightening a check after
watching a run benefit from it is the same move as loosening one, and the
two points are noise beside the gap they sit in. It belongs in v4.

**The 19/100 above stands.** It was not re-scored under the repaired
scorer, and it must not be: re-grading a recorded run after changing the
grader is the thing the honesty conditions exist to prevent. The fixes
apply to the next run, which starts from a clean fixture.

## The manual experiment (pre-registered 2026-09-20, before either run)

Written before the runs it describes, for the same reason the honesty
conditions exist: a hypothesis recorded after the result is not a
hypothesis.

**The claim being tested.** Every model pointed at this harness was trained
as a coding agent. That training buys a loop -- read, change, run the tests,
repeat -- in which the plan is one step deep and something outside the model
says when it is done. This job is two hundred steps deep and nothing says
when it is done. The control scores 90/90, so the harness can express the
work; the schemas already say how to call each tool. What no part of the
system has ever supplied is the *order*: what to build first, which idiom
turns a phase into one call instead of a thousand, and when the job is over.

**The intervention.**

1. One worked example in every tool description. PRD section 6 asked for
   "what + NOT + when + 1 good/bad example" in each tool; three of the four
   shipped and the example never did. This lands in BOTH arms.
2. A `manual` tool -- five playbooks (index, excel-analysis, word-report,
   powerpoint-deck, cross-app) distilled from `load.txt`, which is the
   worked 90/90 solution, rewritten in the tool vocabulary the model sees
   rather than the CLI vocabulary the script is written in. Plus one system
   prompt line telling the model to call it. Arm B only.

The manual is a tool rather than a preamble because of cost: the surface is
~2,240 tokens with it off and ~2,460 with it on, and the playbooks are 340
to 1,205 tokens each, pulled only when asked for. A preamble big enough to
teach the job would crowd out the job on a free-tier context.

**The arms.** Same model (`task deep`, whatever `AGENT_MODEL_REASONING` names),
same brief, same rubric, same `AGENT_MAX_STEPS=120`, a clean fixture from
`setup.ps1` before each.

    A   AGENT_MANUAL=0    tool examples, no playbooks, no manual on the surface
    B   default         the same, plus the manual

**What this can and cannot conclude.** It isolates the playbooks, and only
those: the per-tool examples ship in both arms, so nothing here measures
them. Neither arm is comparable to the four void results above -- those were
scored through a broken instrument and are not a baseline. n=1 per arm, on a
free tier, which means a large gap is worth following and a small one is
noise.

**What would falsify it.** If B scores about what A scores, the plan was not
the binding constraint and the manuals are 3,400 tokens of decoration. That
result gets recorded here with the same prominence as the other one.

No check in the rubric above is added, removed or loosened for this. The
scorer is `score-v3.ps1` at the same revision that graded the control.

### Result: the manual lost, 19 to 34

    A   AGENT_MANUAL=0   34/100   110 executed, 9 refused, budget spent in Word
    B   default        19/100   116 executed, 3 refused, budget spent in Excel

Recorded as it came out. The prediction was that the plan was the binding
constraint; at a 120-step budget that is not what happened.

**What the manual demonstrably did.** It was called unprompted, first move,
both times -- `index` then `excel-analysis` -- and the run then followed the
playbook idiom for idiom:

    A  scorecard built by 134 one-cell writes of hand-computed values
       X4  0/4   formula cells 0, pasted 134
       X5  0/5   12 of 66 metrics within 1%

    B  four derived columns filled from four calls, 1,033,692 cells
       X4  4/4   formula cells 143, pasted 0
       X5  0/5   0 of 66 -- see the scorer defect below

That is the intervention working on the thing it was aimed at. A
hand-computed number pasted into a sheet is the failure the whole
playbook exists to prevent, and it stopped.

**What it cost.** Doing each phase properly is slower. Arm B spent all 120
steps inside Excel and reached neither Word nor PowerPoint; arm A, working
sloppily, got through Excel and 165 paragraphs into Word. Sixty of the
hundred points live outside the workbook, so the careful run scored less.
Both arms ended on `step budget spent`.

**Therefore the experiment as designed cannot answer the question it
asked.** With both arms truncated by the same budget, the comparison
measures how far a run gets in 120 steps, not whether the manual helps it
do the job. The budget, not the plan, is what bound both. That is a defect
in the experiment, mine, recorded here rather than quietly re-run at a
number that flatters the change.

**A scorer defect this run exposed, recorded and NOT repaired.** X3 reports
the scorecard was found on sheet `Monthly` in both arms. It was not the
scorecard. The "candidate carrying the most numbers" heuristic -- itself a
fix from an earlier run -- prefers an 11x12 monthly grid to an 11x7
scorecard, so X4, X5 and X6 were graded against the wrong sheet in both
arms. Arm B's real `Scorecard` sheet is correct, spot-checked against
ground truth in live Excel:

    Whitehorn Multi-Service Centre   2,558,803   truth 2,558,802
    Southland Leisure Centre         1,147,494   truth 1,147,494
    formulas =SUMIF / =MEDIAN(IF(...)) / =PERCENTILE.INC(IF(...),0.95)

Ten points, sitting in a sheet the scorer never opened. The rule holds
anyway: re-grading a recorded run after changing the grader is the move the
honesty conditions exist to forbid, and it forbids it hardest when the
change would help the result I hoped for. **19/100 and 34/100 both stand.**
The heuristic goes in v4, with the run that exposed it.

Two genuine model errors in arm B, neither the manual's doing: all twelve
columns of its `Monthly` sheet carry `month=1`, so every month shows
January's total; and it reached for `find` on the shell, which was refused
before a human was asked.

**What is actually established.** The manual changes behaviour in the
direction it was written to, measurably and on the first try. Whether that
is worth points is unknown, because neither arm was given enough budget to
finish the job. The next experiment is the same pair at a budget that lets
a run reach the end -- a new experiment, pre-registered like this one, not
a re-run of this one with a friendlier setting.

## Experiment 2 (pre-registered 2026-09-20, before either run)

Experiment 1 could not answer its question: both arms ran out of budget, so
it measured how far a run gets in 120 steps. Two things change, and both
change what is being tested, so this is a new experiment rather than a
re-run.

**The recipes are gone.** Experiment 1's manual held phase-by-phase plans
for this exact job, naming the sheets and columns the rubric looks for.
That is a plan written by the person grading the run, and a model following
it demonstrates nothing about its own planning -- it would score and mean
nothing. The manual is now five reference pages describing what the verbs
do to a live application: the fill-down idiom, range-anchored charts, the
pivot's inability to group dates, slide-note addressing. Nothing in it says
what to build or in what order, and a test asserts that: no page may
mention the fixture, its subject, its sheets or a phase order.

**The model keeps its own plan, and can see the clock.** The gap experiment
1 exposed was not knowledge, it was state. Nothing in the loop held the
shape of the job between turns, and the step budget -- enforced since the
beginning -- was never shown to the model. Arm B polished the workbook
until the budget was gone because nothing told it a clock was running.

Added:

- a `plan` tool: the model records its own steps, in its own words, and
  ticks them off. Never written by the harness, and not counted as work, so
  a run that plans and then narrates is still caught by the idle nudge.
- a per-turn status note, one self-replacing system message: what is open,
  which handles are still untouched, the model's plan with its ticks, and
  `Step 74 of 300, 226 left`.

`AGENT_PLAN=0` takes both off, as `AGENT_MANUAL=0` does the manual.

**The budget goes to 300.** The scripted control does the whole job in 217
operations. A model that also has to look at the data, think, and correct
itself cannot do it in 120, and both arms of experiment 1 proved that by
stopping mid-job. 300 is the control plus room to be wrong.

**The arms.**

    A   AGENT_MANUAL=0 AGENT_PLAN=0    the plain harness, tool examples only
    B   default                    manual pages + the model's own plan

Same model, same brief, same rubric, `AGENT_MAX_STEPS=300`, a clean fixture
from `setup.ps1` before each.

**What would falsify it.** If A and B land in the same band, then neither
the reference pages nor the plan-and-clock were the binding constraint, and
the honest conclusion is that a free model cannot hold a job this size
whatever scaffolding it is given. That goes in the table with the same
prominence as any other result.

**What this still cannot conclude.** n=1 per arm, one model, free tier. The
intervention is two changes at once -- reference pages and plan state -- so
a difference does not say which half earned it. Splitting them is a third
experiment, and only worth running if there is a difference to split.

No check in the rubric is added, removed or loosened. The known scorer
defect from experiment 1 -- X3 picking the widest numeric sheet rather than
the scorecard -- is NOT repaired for this run, so experiment 2 is graded by
exactly the grader that produced 34 and 19.

## Experiments 2 and 3, and why both are void

Recorded rather than deleted. A rubric that quietly loses its wrong answers
cannot be checked.

    exp 2  120 -> 300 steps        A  9/100    quit at step 9 by narrating
                                   B  aborted, harness changed under it
    exp 3  after the layer split   A 39/100    142 steps, 63 refused, 413
                                   B 13/100    101 steps,  7 refused, 413

Arm A of experiment 3 is the best score this project has produced, and the
whole gain from 8-19 came from repairing the instrument rather than from
anything clever.

**Both arms of experiment 3 are void, for two reasons found afterwards.**

*They did not run the model they name.* Both switched off
`nemotron-3-ultra` at step ~15 -- arm A at line 40 of 231, arm B at line 53
of 130 -- and spent the rest of the run on `ling-3.0-flash-vl`. The trigger
was one 220-byte non-completion. There was no backoff anywhere in the
harness: `08-production-grade.md` lists opencode's exponential backoff as
ported, and only the model-fallback half of it had landed, so a single blip
permanently demoted the run to a weaker slot. A free-tier limit is per
minute far more often than per day, and waiting two seconds would have kept
the model.

*They both died of their own transcript.* `413 Request too large` at steps
142 and 101, with nothing anywhere pruning the message list. The scaffolding
arm dies sooner because the manual pages and the per-turn status note eat
the same context, so the comparison measured context budget rather than
scaffolding.

Four fixes followed, all before experiment 4 and all with tests:

- transcript compaction: old tool results pruned to 300 chars, never
  removed, the newest twelve messages untouched, and the model told its
  history was shortened so it re-reads rather than trusting a stale number;
- backoff: the same model is asked again after 2s, 6s and 15s before the
  run falls through to another slot;
- a non-completion reports its body, not just its length -- the diagnosis
  had been discarding the only evidence of what those 220 bytes were;
- tool-name recovery: providers glue a model's reasoning into
  `function.name` (`"...</think><tool_call>read"`), and `struct` verbs
  arrive as tool names. Twenty-nine of arm A's sixty-three refusals were
  one of those two, every one a call the harness could have run.

The last of those raises arm A's score on its own, so experiment 4 is not
comparable with the table above. It is pre-registered separately.

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
| 2026-09-19, `ling-3.0-flash-vl:free` | **9/100** | Asked for `qwen3.8-27b`, which was rate-limited on the first call, so the fallback chain ran it on ling instead. One successful operation, a read. Reached for `echo` and `python3`, both refused. Then answered "I'll start by exploring the dataset... in parallel" and the turn ended. |
| 2026-09-19, `deepseek-v4-flash-0731:free` | **8/100** | Both early-exit paths patched before this run. One successful operation, a read, then an empty completion. Nudged, and returned a second empty completion. |
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
