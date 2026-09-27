# Tools reference

Every caller — Syn's own agent loop, the CLI and any MCP client — gets the
same six document operations, with the same names, arguments and JSON
Schemas. They are defined once, in `core/src/tools.rs`, and the MCP surface
is generated from that file.

| Operation | What it does |
|---|---|
| `read` | Read part of an open document. |
| `write` | Put values or formulas into cells, a paragraph or a slide. |
| `format` | Change how something looks: fonts, fills, alignment, spacing. |
| `struct` | Structural change: insert, delete, sort, add a sheet or slide, charts, tables… (`verb` picks which). |
| `export` | Write a copy to a file, or return a summary. |
| `undo` | Take back the last change Syn made to one document. |

The loop also has `shell` (run one allowlisted program after a human
approves it) and two services it answers itself, `plan` and `manual`. MCP
clients get `status`, `open` and `manual` in addition to the six operations;
see [mcp.md](mcp.md).

Schemas are closed: unknown fields and unknown verbs are refused, and every
string has a length cap. Oversized input is refused, never truncated.

## Handles and selectors

A document is addressed by a **handle**, `app:file:unit`, which `open` (or
the CLI's `attach`) returns:

| App | Handle | Selectors |
|---|---|---|
| Excel | `excel:plan.xlsx:workbook` | `Sheet1!A1:D10`; `'Q3 sales'!B2` when the sheet name has a space; `data!5:7` (rows), `data!C:E` (columns); a bare sheet name for the used range |
| Word | `word:report.docx:body` | `body` (the whole text, numbered); `p3` (one paragraph); `p3:p9` (several). `p0` is the first paragraph. `t2` is the second table, `t2.r1` its first row (`t2.r2:r9` several), `t2.c3` a column |
| PowerPoint | `ppt:deck.pptx:deck` | `deck`; `s3` (slide 3, the title for `write`/`format`); `s3.body`; `s3.notes` |
| A window | `ui:Calculator::self` | `:tree`; `id=num7Button`, `name=Seven`, `type=Button`, joined with `,` |
| A web page | `web:Example Domain::doc` | CSS: `h1`, `#total`, `table tr:nth-child(2)`; `body` for the page text |

Small mistakes are resolved when the meaning is certain: a bare file name
(`plan.xlsx`), another unit of the same document (`excel:plan.xlsx:Sheet1`),
`powerpoint:` for `ppt:`, and a different letter case all find the one open
document they can mean. Anything ambiguous is refused with the list of open
handles.

## read

```
read {"handle":"excel:plan.xlsx:workbook","selector":"data!A1:C5"}
```

- **Excel** returns up to 200 cells as values, in the same `a|b;c|d`
  encoding `write` takes, so what is read can be written back. A larger range
  returns only its shape. To compute over a large sheet, write a formula and
  read its one-cell result.
- **Word** returns numbered paragraphs, each tagged with its style when it
  has one: `paras=12 | p0 [Title]: Site visit | p1: Prepared for …`. A long
  document stops after 60 paragraphs (or 6,000 characters) and names the
  range to read next. A table is one entry naming the paragraphs its cells
  take and which table it is: `p9:p68 [table t1, 12x4]: Site|Records;North|24,869;…`
  (Word counts every cell, and the end of every row, as a paragraph).
  `export` summary lists the tables the same way.
- **PowerPoint** `deck` lists the slides and their titles; `s3` lists a
  slide's text; `s3.notes` its speaker notes.

Everything read is returned as untrusted data (see [security.md](security.md)).

## write

```
write {"handle":"excel:plan.xlsx:workbook","selector":"Summary!B2:B12",
       "values":"=SUMIF(data!$A$2:$A$99,$A2,data!$E$2:$E$99)"}
```

`values` is cells joined by `|` and rows by `;` (`a|b;c|d`); `\|` is a
literal pipe. A value starting with `=` is a live formula. One value written
to a multi-cell range fills the range, with references stepping per row the
way Excel's fill does. Formulas use English function names and commas.

- **Word:** `selector` `p3` replaces that paragraph's text.
- **PowerPoint:** `s3` is the title, `s3.body` the bullets (`first|second|>detail`,
  a leading `>` for each level of indent), `s3.notes` the speaker notes.

Writes over the bulk cap are refused.

## format

`style` is `key=value` pairs joined by `;`. Unknown keys are refused.

| App | Selector | Keys |
|---|---|---|
| Excel | a range, rows or columns | `bold`, `italic`, `underline`, `strike`, `size`, `font`, `color`, `fill` (hex), `numberFormat`, `width`, `height`, `autofit`, `autofitSheet`, `wrap`, `merge`, `border`, `align` (left/center/right), `valign` (top/center/bottom), `indent`, `rotate` (degrees), `hidden` (hide the rows or columns named), `freeze` (freeze panes above and left of the selector) |
| Word | a paragraph `p3` or a range `p3:p9`; a table `t2`, its rows `t2.r1` or `t2.r2:r9`, its columns `t2.c3` or `t2.c2:c4` | `style` (e.g. `Heading 1`), `bold`, `italic`, `underline`, `size`, `font`, `color`, `highlight` (yellow, green, cyan, pink, red, blue, gray, none), `fill` (a paragraph or cell colour, or none), `align`, `spaceBefore`, `spaceAfter`, `lineSpacing` (1.5), `indent`; on a whole table also `tableStyle` (e.g. `Grid Table 4 - Accent 1`), `banded` and `header` (1 or 0), `autofit` (content, window or fixed) |
| PowerPoint | `s3` (title) or `s3.body` | `bold`, `italic`, `underline`, `size`, `font`, `color`, `align` |

```
format {"handle":"excel:plan.xlsx:workbook","selector":"Summary!A1:H1",
        "style":"bold=1;fill=#1F4E79;color=#FFFFFF;align=center"}
```

## struct

`verb` chooses the change. A verb an app does not have is refused before the
application is asked, with the list of verbs that app does have.

### Every app

| Verb | Fields | Notes |
|---|---|---|
| `find` | `text`, `selector` (Excel: a sheet or range) | Where the text appears. |
| `replace` | `text`, `with`, `selector` (Excel) | Every occurrence; says how many. |
| `delete` | `selector` | Excel rows/columns/cells, Word `p3` or `p3:p5`, PowerPoint `s3`. |
| `pageSetup` | `style`, `selector` (Excel: the sheet) | `orientation=landscape`, `paper=A4` or `Letter`, `margin=0.75` (inches); Excel adds `fitWide`, `fitTall`; a deck takes `size=16:9`, `4:3`, `16:10`, `A4`, `Letter` and `orientation`. |
| `pageNumbers` | `text` (optional footer text) | Excel: every sheet's footer. Word: the footer. PowerPoint: slide numbers. |
| `picture` | `text` (file path), `selector`, `name` | Excel: the range it fills. Word: `name` is the width in points. PowerPoint: `selector` is the slide, `name` the box `left,top,width,height` in points. |
| `save` | — | Writes the document to its own file, in its own format: never a Save As. Refused for a document never saved, one open read-only, and a CSV (it keeps one sheet's values); `export` writes a copy instead. |
| `close` | — | Closes a document Syn opened, once saved. Refused with unsaved changes and for anything the user already had open. Never quits the application. Its handles leave the registry. |

### Excel

| Verb | Fields | Notes |
|---|---|---|
| `addSheet` | `name` | |
| `sheet` | `selector` (sheet), `action`, `name` | `rename`, `delete`, `copy`, `hide`, `show`. |
| `insert` | `selector` | Rows `data!5:7`, columns `data!C:E`, or cells (pushed down). |
| `sort` | `selector`, `name` (header), `rule` | Header row first; `asc` or `desc`. |
| `filter` | `selector`, `name` (header), `rule` | What to keep: `North`, `>100`, `<>0`; empty clears. |
| `dedupe` | `selector`, `name` | Headers that must all match, joined by `|`; every column when empty. |
| `copy` | `source`, `at` | Values, formulas and formats to the top-left cell `at`. |
| `validate` | `selector`, `rule` | `list=Yes,No,Maybe`, `whole=1..10`, `decimal=0..1`. |
| `table` | `source`, `name` | A real Excel Table. |
| `name` | `name`, `at` | A named range. |
| `conditional` | `selector`, `rule` | `dataBar`, `colorScale`, `iconSet`, `top10`, `greaterThan=N`, `lessThan=N`. |
| `pivot` | `source`, `rows`, `cols`, `values`, `at` | Fields are header names; the destination sheet must exist. |
| `slicer` | `rows` (the field), `name` (the pivot; the only one when empty), `at` | Build the pivot first. |
| `chart` | `kind`, `source`, `at`, `title`, `style` | `kind`: line, bar, column, pie, scatter, area, doughnut, stackedColumn, stackedBar, lineMarkers, radar. `at` as a range sizes the chart to it. `style`: `legend=0`, `gridlines=0`, `xTitle=…`, `yTitle=…`, `dataLabels=1`. |
| `comment` | `selector`, `text` | A note on a cell. |
| `link` | `selector`, `text` (address), `title` | |
| `header` | `name` (`header`/`footer`), `text`, `selector` | Centre header or footer; every sheet when no sheet is named. |
| `macro` | `action`, `name`, `code`, `title` | VBA; off unless enabled, see below. |

### Word

| Verb | Fields | Notes |
|---|---|---|
| `insertParagraph` | `text`, `name` (style), `at` | Appends, or goes before paragraph `at` and becomes it. Styles by their built-in names (`Heading 1`, `Title`, `Quote`, `List Bullet`, `List Number`) work in any language of Word. |
| `insertTable` | `rows` (`a|b;c|d`), `name` (table style), `at` | Appends, or goes before paragraph `at`. The cells are Normal text whatever they were put in front of. The reply names the table and its paragraphs: `table t2 added, 3x4 [Table Grid], as p9:p24`. |
| `delete` | `selector` | A paragraph `p3`, a range `p3:p5`, or a whole table `t2`. |
| `embedChart` | `from` (a workbook handle), `source` (`Summary!2`, the second chart on Summary), `name` (width in points), `at`, `style` | A real chart from an open workbook, still a chart that can be edited, linked to the workbook so it follows its numbers (`link=0` embeds a copy instead). `from` must be a handle this session opened, and passes the same gates as any Excel call. The chart travels by the clipboard; text you had copied is put back. |
| `header` | `name` (`header`/`footer`), `text` | Replaces what is there; add `pageNumbers` after a footer. |
| `pageBreak` | `name` (`page`/`section`) | |
| `contents` | `title` | A table of contents; calling it again refreshes it. |
| `comment` | `selector` (`p3`), `text` | |
| `link` | `selector` (`p3`), `text` (address) | |

### PowerPoint

| Verb | Fields | Notes |
|---|---|---|
| `createSlide` | `title`, `bullets` (`a|b|>sub`), `name` (layout) | Layouts: `title`, `titleContent`, `sectionHeader`, `twoContent`, `comparison`, `titleOnly`, `blank`. |
| `duplicateSlide` | `selector` | The copy lands just after. |
| `moveSlide` | `selector`, `at` | `at` is where it goes; `s1` for first. |
| `textBox` | `selector`, `text`, `name` (box), `style` | Box is `left,top,width,height` in points; `style` takes `size`, `bold`, `italic`, `font`, `color`, `align`. |
| `insertTable` | `selector`, `rows`, `name` (box) | |
| `theme` | `text` (a `.thmx` or `.potx`) | Every slide. |

### Windows and web pages

`invoke` presses a control (`action`: `invoke`, `click`, `toggle`, `select`,
`expand`, `collapse`, `focus`). `transfer` (`from`, `selector`, `title`)
copies typed data between two handles with its provenance recorded.

### VBA

`macro` writes, runs, reads or lists VBA modules in an Excel workbook:
`action=write` with `name` (the module) and `code` (replaces the module),
`action=run` with `title` (the macro), `action=read` with `name`,
`action=list`. It is refused unless `AGENT_VBA=1` is set, and Excel must have
*Trust access to the VBA project object model* enabled. `run` saves a copy of
the workbook first, because running VBA clears Excel's own undo list. A
built-in filter refuses obviously dangerous calls (`Shell`, `CreateObject`,
`Kill`, `SendKeys`, `Declare`, …); it is a safeguard against careless code,
not a security boundary. Code that starts itself (`Auto_Open`,
`Workbook_Open`, …) and code with `MsgBox` or `InputBox`, which would wait
for a click nobody makes, is refused when it is written.

Only `write`, `read` and `list` need the Trust Center setting; `run` calls a
macro already in the workbook, and always this workbook's (by its full
name, never a same-named macro elsewhere). A macro that fails does not hang
the run: VBA's error box is read, closed as if End (or OK, for a compile
error) had been pressed, and its words come back as the error, e.g.
`stopped with a VBA error: Run-time error '11': Division by zero`. The model
is told in its environment whether VBA is on for the session.

A macro written into an `.xlsx` lives only while the workbook is open: the
format cannot hold VBA, so `save` keeps what the macro did and drops the
macro itself.

## export

Always writes a **copy**: the open document stays where it is, unsaved
changes included in the copy.

| App | Formats |
|---|---|
| Excel | `xlsx`, `csv` (one sheet, `sheet`; the first by default), `pdf`, `png` (every chart, one file each: `<stem>-<sheet>-<n>.png`) |
| Word | `docx`, `pdf` |
| PowerPoint | `pptx`, `pdf`, `png` (every slide: `<stem>-s1.png` …) |

`summary` (no `path`) returns the sheets and their used ranges, a document's
headings, or the slide titles.

```
export {"handle":"excel:plan.xlsx:workbook","format":"png","path":"C:\\out\\fig.png"}
```

## undo

```
undo {"handle":"excel:plan.xlsx:workbook"}
```

Takes back the last change Syn made to that document, newest first, up to 20
per document. Each application gets the undo it can really do:

- **Word** — Word's own undo list, with each Syn call recorded as a single
  entry.
- **Excel** — COM changes do not join Excel's undo list, so Syn copies the
  cells a call will change to a hidden scratch workbook first, and removes
  the sheets, charts, pivots, tables, names and pictures a call adds.
  Deleted rows and sheets come back, but formulas elsewhere that pointed
  into them keep their `#REF!`.
- **PowerPoint** — a copy of the deck is saved before each call; undo puts
  back the slides the call added, removed, moved or changed, and the slide
  size.

Undo is refused, with the reason, when someone else has edited the document
since Syn's change (undoing would take their work too), and after a `macro`,
which can change anything.
