# Natural sessions

Four scripted conversations, 50 messages each, written the way a person
works: one or two things per message, building on what is already there,
with questions, second thoughts, undos and "put it back" along the way. Each
moves from Excel to a report in Word to a deck in PowerPoint.

| Script | File | Size |
|---|---|---|
| `hotels.md` | `Hotel_Bookings_Demand.csv` | 119,390 rows |
| `oil.md` | `US_Crude_Oil_Import.csv` | 483,053 rows |
| `electricity.md` | `Global_Electricity_Production.csv` | 121,074 rows |
| `ev.md` | `electric_vehicle_population_data.csv`, from `Electric_Vehicle_Population.zip` | 181,458 rows |

The data files are real, published datasets from the Evals project
(`Evals/dataframe/`), chosen because they are big and have the faults real
data has: text placeholders for empty cells, codes instead of names,
outliers, subtotals mixed in with the rows they total, a partial last year.
They are not vendored here. Copy the one a script needs into the chat's
workspace folder (extract the EV file from its zip first).

The capability test (`tests/capability/`) asks for everything in one brief
and scores the result. These do the opposite: no single message asks for
much, and what they show is whether a model can hold a real working session
together over fifty turns.

## How to run one

1. A fresh chat, its workspace set to a folder holding the script's data
   file and nothing that a previous run made.
2. Send the messages one at a time, in order, each only after the last has
   finished. Type them into the console, or send them through it
   (`say <message>`): either way the model sees the same thing.
3. Leave the model alone between messages. Answering its questions is fair;
   telling it how to do the work is not.

## Rules, so the result means something

- **The prompts name no tool, verb, selector or handle.** They mention cells
  and sheet names only where a person would.
- **Never reword a prompt after seeing a run.** A prompt that turns out to
  be ambiguous stays as written, and the run is read with that in mind. A
  change goes in as a new version of the script, with the reason, and the
  runs made under the old one stay as they are.
- **Check numbers against the data, not against the model's account of
  them.** Each script's header says what the data holds, including the
  faults a careful analyst should find.

## What they are expected to run into

Some prompts ask for things Syn cannot do yet. They are left in on purpose:
a person would ask, and how the run says no is part of what is being tested.
Known when these were written (a new Word document or deck was on this list
until `open` learned `create`):

- **Macros and buttons.** VBA is off unless `AGENT_VBA=1` (the gate in
  `Runner::pump`), so the button prompts are refused by default.
- **A pivot grouped by month, or with several value fields.** Not supported
  (`tests/capability/SPEC-v3.md`, "What the engine cannot do yet").
- **Probably unsupported, not yet checked:** sheet protection, hiding
  gridlines, a month timeline, and slide animations.
