# 10 — Adding tools without Office

Date: 2026-09-20
Status: how-to. The pieces described here exist and are tested.

The problem this solves: adding a verb touches Rust and C#, and until now the
only way to find out whether the two agreed was to drive real Office on a
Windows machine with Microsoft 365 installed. That made every change a trip
to one particular desk, and made a cloud session's work a guess.

You can now build a tool anywhere, prove most of it anywhere, and leave
exactly one step for the machine with Office on it.

## What each tier can prove

| Tier | Needs | Proves |
|---|---|---|
| 1 | nothing | the loop, the parser, the schemas, and that both sides of the wire agree |
| 2 | an API key | how a model behaves over a long run, against in-memory documents |
| 3 | Windows + Office | that the COM call is right and the document ends up correct |

Tier 1 is `cargo test` on any platform — Linux, macOS, a cloud sandbox. It is
already 285 tests and CI runs it on three operating systems.

Tier 2 is the part people forget exists: `attach` **without** `live` binds a
handle to the in-memory document model. `read`, `write`, `format`,
`insertParagraph`, `insertTable`, `addSheet`, `transfer` and `export` all
work there with no Office at all. What you lose is formula evaluation and
the Office-only verbs (pivot, chart, slicer, conditional, picture), so tier 2
answers "does the model drive this sensibly for three hundred steps" and
never "is the number right".

Tier 3 is the only step that needs the desk.

## The contract that makes it safe

`core/tests/wire_contract.rs` reads both languages as text and fails if they
disagree:

- every method `hand.rs` can put on the wire has a handler in
  `sidecar-csharp/Host/Program.cs` or `Uia/Program.cs`;
- every `struct` verb offered to the model reaches the wire at all;
- both sidecar files still parse as expected, so a rename cannot make the
  test pass by checking nothing.

It needs no Office, no dotnet and no network, and it runs in under a second.
A verb added on one side only now fails in CI instead of on someone's desk.
Methods deliberately left unimplemented are listed in
`DELIBERATELY_UNIMPLEMENTED` with the reason — currently just `undo`, whose
live behaviour belongs to the sidecar's `.bak` and the application's own
undo stack.

## Adding a verb, start to finish

1. **`core/src/ops.rs`** — a variant on `StructArgs` (or a new `Call`).
2. **`core/src/tools.rs`** — the schema: add it to the `struct` enum and to
   `STRUCT_VERBS`. A test ties those two together, so forgetting one fails.
   Keep the closed-schema rules: `additionalProperties:false`, a cap on every
   string, refuse rather than truncate.
3. **`core/src/hand.rs`** — a case in `envelope_for` producing the wire line.
   Geometry and style ride in the short `args`; prose and grids ride in
   `payload`, because `args` is for the short fields that describe the long
   one.
4. **`sidecar-csharp/Host/Program.cs`** — a case in the right `method
   switch`. Late-bound COM, and remember PowerPoint's tri-states: `Visible`,
   `DisplayAlerts`, `HasTextFrame` and `HasTable` are `MsoTriState`, so the
   boolean form fails the *cast*, not the call.
5. **`core/src/bin/cli.rs`** — a command, if a human should be able to drive
   it by hand. Convention: geometry and style are one token and come
   **before** free text, because free text runs to the end of the line.
6. **`core/src/manual.rs`** — a line on the relevant page if the verb has a
   non-obvious idiom. Reference only: nothing in there may name a fixture or
   prescribe an order (a test enforces that).
7. **Tests.** Tier 1: the schema, the envelope, the refusal path. Then
   `cargo test` and `cargo clippy --all-targets -- -D warnings`, both of
   which CI runs on three platforms.

Steps 1–3 and 5–7 are pure Rust and can be done from anywhere. Step 4 is C#
that compiles on Windows without Office — CI's `sidecar` job builds it, so a
cloud branch still gets a compile check.

What remains for the desk: running the verb against a live document once.

## The harness opens its own documents

`open` is the one request that names no open document, because it is how one
becomes open. Every other method starts by finding the document and fails if
it is missing.

    cli.exe
      hand hand-word word
      open word D:\path\to\report.docx

It starts the application if it is not running, opens the file, makes it
visible, and is **idempotent** — a file already open is returned as it is,
because Office answers a second `Open` of the same path with a read-only
copy and a run would then write into the copy. It never closes and never
saves anything: the sidecar's rule is that it does not touch what it did not
start.

This is why it matters: a capability run died at step 87 with
`com: doc not open for word:solar-memo.docx:body`, retried, and was stopped
by the doom-loop gate. The document had gone and the run had no way to say
so. A session can now set itself up instead of depending on somebody having
opened three files by hand first.

## Rules of thumb

- **The six primitives are the product.** A new capability is almost always
  a `struct` verb, not a seventh op.
- **World tools and loop services are different layers** (`docs/09`). A tool
  that reaches a document goes in `tools.rs` and is covered by the security
  fingerprint; one the loop answers itself goes in `looptools.rs` and is not.
- **Anything a cloud session cannot verify, say so in the PR.** "Compiles,
  tier 1 green, needs a live check on Word" is a complete and honest hand-off.
