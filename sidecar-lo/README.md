# sidecar-lo — the office helper for machines without Microsoft Office

`lo_host.py` speaks `office-rpc/1` exactly as `sidecar-csharp/Host`
(office-host.exe) does, with the same method names and, wherever it can, the
same replies word for word. It drives LibreOffice through UNO instead of
Office through COM, and listens on a Unix socket instead of a named pipe.
On Linux and macOS, `mcpgate`, the REPL (`hand hand-excel excel`) and the
console all use it the same way they use office-host on Windows.

It exists so the whole stack can be tested against a real office engine
that evaluates formulas and writes real `.xlsx`, `.docx` and `.pptx` files,
anywhere, in CI. It does not test the C# helper, COM or Excel itself; that
is still tier 3, on Windows (`docs/development.md`).

## Requirements

LibreOffice (Calc, Writer, Impress) and its Python bridge:

```
sudo apt-get install libreoffice-calc libreoffice-writer libreoffice-impress python3-uno
```

`python3-uno` belongs to the system Python, so point Syn at that one if
`python3` on your PATH is another: `AGENT_PYTHON=/usr/bin/python3`.

## Running it

Nothing to start by hand. With `AGENT_PIPE_EXCEL=hand-excel` (and `_WORD`,
`_PPT`) in `.env`, `mcpgate`'s `open` starts one helper per app when it is
first needed, and each helper starts its own headless LibreOffice. When
`mcpgate` exits, it stops the helpers, and they stop their LibreOffice.

By hand, for debugging:

```
python3 sidecar-lo/lo_host.py --pipe hand-excel --app excel --trace
```

`AGENT_LO_VISIBLE=1` with a display shows the LibreOffice windows instead of
running headless.

## What it does, and what it refuses

Every app: `open`, `read`, `write`, `export` (a copy; `summary` needs no
path), `undo`, `find`, `replace`, `delete`.

| App | Also handled |
|---|---|
| Excel (Calc) | `format`, `insert`, `sort`, `copy`, `sheet` (rename, delete, copy, hide, show), `addSheet`, `chart` (line, bar, column, pie), `comment`, `header`, `pageNumbers`; export xlsx, csv (any sheet), pdf. Formulas are written in Excel's English syntax. |
| Word (Writer) | `read` as numbered text (`body`, `p3`, `p3:p9`), `insertParagraph` (with style, at a position), `insertTable`, `pageBreak`, `comment`, `header`; export docx, pdf. |
| PowerPoint (Impress) | `createSlide` (layouts, bullets, `>` sub-bullets), `write` (`sN` title, `sN.notes`), `duplicateSlide`, `textBox`, `pageSetup` (slide size); export pptx, pdf. |

Every other verb is refused with `unsupported <app>.<method> on the
LibreOffice helper`, never faked: among them pivots, slicers, filters,
validation, conditional formats, VBA, contents, pictures, page setup outside
decks, PNG exports, moving slides and themes.

Undo uses LibreOffice's own undo list for Calc and Writer, and a record of
its own for decks (Impress changes made through the API do not reach
LibreOffice's undo list); undoing a deleted or moved slide is refused.

`core/tests/wire_contract.rs` checks that every method handled here is one
the Rust side sends, and that the core set above is still handled.

Up to eight clients can be connected at once (the console, MCP clients, a
terminal). LibreOffice handles one request at a time, in arrival order. A
helper refuses to start if another one is already answering on its pipe.
If a helper dies without stopping its LibreOffice (SIGKILL), the next helper
on the same pipe takes over that LibreOffice instead of starting a second
one that would find the documents locked, the way office-host attaches to a
running Excel.

Differences a model might notice: a LibreOffice deck always has at least one
slide, so an "empty" deck arrives with one blank slide, and the first
`createSlide` fills it. Word's table style names have no LibreOffice
equivalent and are reported as left as-is.

## Tests

- `tests/test_lo_live.py` runs the real `mcpgate` against LibreOffice:
  opening, formulas, fill-down, charts, a memo, a deck, notes, the mistake
  messages, and checks the exported files by unzipping them. It skips
  without LibreOffice unless `SYN_REQUIRE_LO=1`, which the `libreoffice` CI
  job sets.
- `make_fixtures.py DIR` writes the documents it opens.
