// Screenshots of the console in the states a human actually sees, for
// judging the design by eye. Nothing here asserts: the specs do that. This
// answers "does it look right", which no assertion can.
//
//   cd core && cargo build --bins
//   cd ../tests/ui && node showcase.mjs [out-dir]
//
// Starts a console of its own on port 7810, stages each state through
// `window.live` -- the same surface the specs drive -- and writes one PNG
// per state per colour scheme into testbed/shots/showcase (gitignored).
// Set PW_CHROMIUM to use a preinstalled Chromium, as the specs do.

import fs from "node:fs";
import path from "node:path";

import { chromium } from "@playwright/test";

import { repo, startConsole } from "./console.mjs";

const out = path.resolve(process.argv[2] ?? path.join(repo, "testbed", "shots", "showcase"));
fs.mkdirSync(out, { recursive: true });

const step = (o) => "RECEIPT step " + JSON.stringify(o);

// A conversation shaped like a real one: a request, the working, an answer
// with the table a model usually answers with, then a follow-up mid-run.
const transcript = [
  { role: "user", text: "Build a per-site scorecard from the solar data and put the top three on a slide." },
  {
    role: "calls",
    text: JSON.stringify([
      { id: "a", function: { name: "read", arguments: '{"handle":"excel:solar.xlsx:data","selector":"data!A1:H6"}' } },
      { id: "b", function: { name: "struct", arguments: '{"handle":"excel:solar.xlsx:data","verb":"addSheet","name":"Scorecard"}' } },
      { id: "c", function: { name: "write", arguments: '{"handle":"excel:solar.xlsx:Scorecard","selector":"Scorecard!B2:B12","values":"=SUMIF(...)"}' } },
      { id: "d", function: { name: "format", arguments: '{"handle":"excel:solar.xlsx:Scorecard","selector":"A1:H1","style":"bold=1"}' } },
      { id: "e", function: { name: "struct", arguments: '{"handle":"ppt:review.pptx:deck","verb":"createSlide"}' } },
      { id: "f", function: { name: "struct", arguments: '{"handle":"excel:solar.xlsx:Scorecard","verb":"addSheet","name":"Scorecard"}' } },
    ]),
  },
  { role: "tool", id: "a", text: "grid data: 6x8" },
  { role: "tool", id: "b", text: "sheet added" },
  { role: "tool", id: "c", text: "11 cells" },
  { role: "tool", id: "d", text: "styled" },
  { role: "tool", id: "e", text: "slide 1" },
  { role: "tool", id: "f", text: "already exists" },
  {
    role: "assistant",
    text:
      "The scorecard is on a new **Scorecard** sheet, every figure a live formula, and the top three sites are on slide 1.\n\n" +
      "## Top sites by output\n\n" +
      "| Site | Total kWh | Share |\n|---|---:|---:|\n" +
      "| Bearspaw Water Treatment Plant | 3,082,638 | 31.3% |\n" +
      "| Whitehorn Multi-Service Centre | 2,558,802 | 26.0% |\n" +
      "| Southland Leisure Centre | 1,147,494 | 11.7% |\n\n" +
      "Two things worth acting on:\n\n" +
      "- Bearspaw alone carries almost a third of the estate, so its downtime is the estate's downtime.\n" +
      "- Winter output is **15x** lower than July's; any target set on an annual average will look missed every December.\n",
  },
  { role: "user", text: "Now write the executive summary into the Word memo." },
];

const labels = {
  a: { text: "Read data!A1:H6 in solar.xlsx", status: "done", app: "excel" },
  b: { text: "Added the sheet Scorecard", status: "done", app: "excel" },
  c: { text: "Wrote formulas into Scorecard!B2:B12", status: "done", app: "excel" },
  d: { text: "Formatted Scorecard!A1:H1", status: "done", app: "excel" },
  e: { text: "Created slide 1 in review.pptx", status: "done", app: "ppt" },
  f: { text: "Tried to add the sheet Scorecard", status: "refused", app: "excel" },
};

const live = [
  step({ label: "Read memo.docx", tool: "read", app: "word", status: "done", detail: "4 paragraphs" }),
  step({ label: "Inserted a heading: Executive summary", tool: "struct", app: "word", status: "done", detail: "p5" }),
  step({ label: "Writing the summary paragraph", tool: "struct", app: "word", status: "running", detail: "" }),
];

async function stage(page, name) {
  for (const scheme of ["dark", "light"]) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(out, `${name}-${scheme}.png`) });
  }
}

const c = await startConsole({ port: Number(process.env.SYN_SHOWCASE_PORT ?? 7810) });
const exe = process.env.PW_CHROMIUM;
const browser = await chromium.launch(exe ? { executablePath: exe } : {});
try {
  for (const [w, h, tag] of [[1440, 900, ""], [420, 860, "-narrow"]]) {
    const page = await browser.newPage({ viewport: { width: w, height: h } });
    await page.goto(c.url);
    await page.waitForFunction(() => !!window.live);
    await page.evaluate(() => window.live.quiet());
    await page.waitForTimeout(300);

    // 1. The first thing anyone sees.
    await page.evaluate(() => window.live.render([], {}));
    await stage(page, "empty" + tag);

    // 2. A finished turn, and a second one running, with the stack up.
    await page.evaluate(
      ([t, l, lines]) => {
        window.live.render(t, l);
        window.live.budget(23, 40);
        window.live.step("RECEIPT say model=deepseek/deepseek-chat open=3");
        for (const x of lines) window.live.step(x);
        window.live.banner("retry", {
          variant: "warning",
          priority: "urgent",
          title: "That model was busy",
          description: "Retrying with qwen/qwen3-coder.",
        });
      },
      [transcript, labels, live],
    );
    await page.evaluate(() => (document.getElementById("scroll").scrollTop = 1e9));
    await stage(page, "run" + tag);
    if (!tag) {
      await page.evaluate(() => (document.getElementById("scroll").scrollTop = 0));
      await stage(page, "run-top");
      await page.evaluate(() => (document.getElementById("scroll").scrollTop = 1e9));
    }

    // 3. The decision point: a shell call waiting for a human.
    await page.evaluate(() => {
      window.live.banner("retry", null);
      window.live.ask?.({ preview: "hostname", why: "to label the report with the machine it was built on" });
    });
    await stage(page, "approval" + tag);

    // 4. The status menu open.
    if (!tag) {
      await page.evaluate(() => window.live.ask?.(null));
      await page.locator("#hands").click();
      await stage(page, "menu");
    }
    await page.close();
  }
} finally {
  await browser.close();
  c.stop();
}
console.log(out);
