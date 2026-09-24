// What a human actually reads during a run.
//
// Offline: no model, no API key, no Office. The page is driven by handing
// `liveStep` the exact lines the CLI prints, so this is a test of the
// rendering and nothing else. It runs in a second and costs nothing, which
// is the only reason it will keep being run.
//
// The regression it exists for: the console used to parse `STEP <tool>:
// <detail>` out of the CLI's prose. Rewording that line to read better in
// the terminal silently stopped the live view drawing tool rows — no
// error, the page just went quiet mid-run. `core/tests/console_contract.rs`
// holds the two sides in step as text; this checks the result on a page.

import { test, expect } from "@playwright/test";

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { startConsole } from "./console.mjs";

// A console of its own, pinned to a log this spec owns and never writes.
// Sharing one meant any other console started by another spec became the
// "newest" run, the shared page followed it, and the timeline this spec
// had just built was wiped out from under it. A rendering spec must not
// depend on what else is running on the machine.
let own;
let url;

/** Feed the page one CLI output line, as the poller would. */
async function say(page, line) {
  await page.evaluate((l) => window.live.step(l), line);
}

/** Every tool row currently on the timeline. */
async function rows(page) {
  return page.$$eval(".act", (els) =>
    els.map((e) => ({
      text: e.querySelector(".nm")?.textContent ?? "",
      status: e.dataset.status ?? "",
      app: e.dataset.app ?? "",
      badge: e.querySelector(".bad")?.textContent ?? "",
    })),
  );
}

const step = (o) => "RECEIPT step " + JSON.stringify(o);

test.beforeAll(async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "syn-render-"));
  const log = path.join(dir, "quiet.log");
  fs.writeFileSync(log, "");
  own = await startConsole({ port: Number(process.env.SYN_RENDER_TEST_PORT ?? 7805), tail: log });
  url = own.url;
});

test.afterAll(() => own?.stop());

test.beforeEach(async ({ page }) => {
  await page.goto(url);
  await page.waitForFunction(() => !!window.live);
  // Let the page finish its own first paint before touching it.
  // `refresh()` repaints the whole timeline from the saved transcript, so
  // a line injected while that is still in flight is wiped by it.
  //
  // Not `networkidle`: the page holds an event-stream open for as long as
  // it is on screen, so "no network for 500ms" is a state this page never
  // reaches. Waiting for the thing that actually matters instead.
  await page.waitForFunction(() => window.live.source !== null || window.live.sse, null, { timeout: 15_000 });
  await page.evaluate(() => {
    window.live.quiet();
    document.querySelector("#tl").textContent = "";
  });
});

test("the page loads without throwing", async ({ browser }) => {
  // The cheapest check there is, and the one that was missing. A single
  // duplicate `const` is a syntax error: the whole script fails to
  // evaluate, every function on the page is gone, and what a person sees
  // is a blank screen with no clue why. That happened, and eleven
  // behavioural specs all reported "timed out waiting for window.live"
  // instead of "the page is broken".
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error" && !/Failed to (fetch|load)/i.test(m.text())) errors.push(m.text());
  });
  await page.goto(url);
  await page.waitForFunction(() => !!window.live, null, { timeout: 10_000 });
  await page.waitForTimeout(500);
  await page.close();
  expect(errors).toEqual([]);
});

test("a model's answer renders as a document, not as its source", async ({ page }) => {
  // The worst thing this page did: a model that had just built a table
  // answered with one, and the page printed `|---|---|`. A heading came
  // out as bold text mid-paragraph and a list came out as bullet
  // characters with no indent.
  const answer = [
    "## Totals",
    "",
    "Added a `Totals` sheet:",
    "",
    "| site | total kwh |",
    "|---|---|",
    "| North | =SUMIF(Sheet1!$A:$A,A2,Sheet1!$C:$C) |",
    "| South | =SUMIF(Sheet1!$A:$A,A3,Sheet1!$C:$C) |",
    "",
    "Next steps:",
    "",
    "- check the **North** figure",
    "  - against Jan and Feb",
    "- then build the deck",
    "",
    "> The formulas are live, not pasted values.",
  ].join("\n");
  await page.evaluate((t) => window.live.render([{ role: "assistant", text: t }], {}), answer);

  const bot = page.locator(".bot");
  // A real table, with the header in a thead and two body rows.
  await expect(bot.locator("table thead th")).toHaveCount(2);
  await expect(bot.locator("table tbody tr")).toHaveCount(2);
  await expect(bot.locator("table tbody tr").first()).toContainText("North");
  // A real heading, a real nested list, a real quote.
  await expect(bot.locator("h2")).toHaveText("Totals");
  // `:scope >` matters: a plain `ul > li` also matches the nested item,
  // which is what a list that is only pretending to nest would give too.
  await expect(bot.locator(":scope > ul > li")).toHaveCount(2);
  await expect(bot.locator("ul ul li")).toHaveCount(1);
  await expect(bot.locator("blockquote")).toContainText("live, not pasted");
  await expect(bot.locator("strong")).toHaveText("North");

  // And none of the source leaks through as text.
  const shown = await bot.innerText();
  expect(shown).not.toContain("|---|");
  expect(shown).not.toContain("## ");
  expect(shown).not.toMatch(/^- /m);
});

test("a model cannot put markup on the page", async ({ page }) => {
  await page.evaluate(() =>
    window.live.render([{ role: "assistant", text: "<img src=x onerror=alert(1)> and <b>bold</b>" }], {}),
  );
  const bot = page.locator(".bot");
  await expect(bot.locator("img")).toHaveCount(0);
  await expect(bot.locator("b")).toHaveCount(0);
  await expect(bot).toContainText("<img src=x onerror=alert(1)>");
});

test("a fenced block keeps its shape and is not parsed as anything else", async ({ page }) => {
  const src = ["Here:", "", "```vba", "Sub Build()", "  ' | not | a | table |", "End Sub", "```"].join("\n");
  await page.evaluate((t) => window.live.render([{ role: "assistant", text: t }], {}), src);
  const pre = page.locator(".bot pre");
  await expect(pre).toHaveCount(1);
  await expect(pre).toContainText("Sub Build()");
  await expect(pre).toContainText("| not | a | table |");
  await expect(page.locator(".bot table")).toHaveCount(0);
  expect(await pre.getAttribute("data-lang")).toBe("vba");
});

test("a tool call reads as a sentence, never as a tool name", async ({ page }) => {
  await say(
    page,
    step({
      label: "Drew a chart on Dashboard!A1:H16",
      tool: "struct",
      app: "excel",
      status: "done",
      detail: "chart added",
    }),
  );

  const [row] = await rows(page);
  expect(row.text).toBe("Drew a chart on Dashboard!A1:H16");
  expect(row.status).toBe("done");
  expect(row.app).toBe("excel");
  // The two things that must never reach a reader.
  expect(row.text).not.toContain("struct");
  expect(row.text).not.toContain("{");
});

test("a refusal and a broken run do not look the same", async ({ page }) => {
  await say(page, step({ label: "Read Sheet1!A1", tool: "read", app: "excel", status: "done", detail: "ok" }));
  await say(
    page,
    step({ label: "Refused to add Scorecard", tool: "struct", app: "excel", status: "refused", detail: "exists" }),
  );
  await say(
    page,
    step({ label: "Stopped reading Sheet1!A1", tool: "read", app: "excel", status: "stopped", detail: "killed" }),
  );

  const all = await rows(page);
  expect(all.map((r) => r.status)).toEqual(["done", "refused", "stopped"]);
  // A call that ran cleanly is not badged at all; the other two are, and
  // differently, because "the application said no" and "the run is over"
  // are different news.
  expect(all[0].badge).toBe("");
  expect(all[1].badge).toBe("refused");
  expect(all[2].badge).toBe("stopped");

  const colours = await page.$$eval(".act .bad", (els) => els.map((e) => getComputedStyle(e).color));
  expect(colours[0]).not.toBe(colours[1]);
});

test("the prose lines are ignored, so rewording one cannot blank the view", async ({ page }) => {
  await say(page, "STEP Read Sheet1!A1:B2");
  await say(page, "REFUSED handle not open");
  await say(page, "DID Read 2 ranges");
  expect(await rows(page)).toHaveLength(0);

  // ...and the data line for the same event does draw.
  await say(page, step({ label: "Read Sheet1!A1:B2", tool: "read", app: "excel", status: "done", detail: "2x2" }));
  expect(await rows(page)).toHaveLength(1);
});

test("a row expands to the detail and starts collapsed", async ({ page }) => {
  await say(
    page,
    step({ label: "Read Sheet1!A1:B2", tool: "read", app: "excel", status: "done", detail: "grid Sheet1: 2x2" }),
  );
  const body = page.locator(".body").first();
  await expect(body).toBeHidden();
  await page.locator(".act").first().click();
  await expect(body).toBeVisible();
  await expect(body).toContainText("grid Sheet1: 2x2");
});

test("twelve calls stay one screen of quiet rows", async ({ page }) => {
  for (let i = 0; i < 12; i++) {
    await say(
      page,
      step({ label: `Read Sheet1!A${i}:B${i}`, tool: "read", app: "excel", status: "done", detail: "2x2" }),
    );
  }
  const all = await rows(page);
  expect(all).toHaveLength(12);

  // The whole point of the row shape: twelve calls must not shout as loudly
  // as the answer. Each row is a single short line, and the block of them
  // is shorter than a screen.
  const heights = await page.$$eval(".act", (els) => els.map((e) => e.getBoundingClientRect().height));
  for (const h of heights) expect(h).toBeLessThanOrEqual(30);
  const block = await page.$eval(".acts", (e) => e.getBoundingClientRect().height);
  expect(block).toBeLessThan(420);
});

test("a long label truncates instead of wrapping the row", async ({ page }) => {
  await say(
    page,
    step({
      label: "Wrote " + "VeryLongSheetName!".repeat(20) + "A1:Z99",
      tool: "write",
      app: "excel",
      status: "done",
      detail: "ok",
    }),
  );
  const h = await page.$eval(".act", (e) => e.getBoundingClientRect().height);
  expect(h).toBeLessThanOrEqual(30);
  const clipped = await page.$eval(".act .nm", (e) => e.scrollWidth > e.clientWidth);
  expect(clipped).toBe(true);
});

test("the run says in one sentence what it did", async ({ page }) => {
  await say(page, step({ label: "Read Sheet1!A1", tool: "read", app: "excel", status: "done", detail: "ok" }));
  await say(page, "RECEIPT did " + JSON.stringify({ text: "Read 2 ranges and wrote into 1 document" }));
  await expect(page.locator(".did")).toHaveText("Read 2 ranges and wrote into 1 document");
});

test("a malformed receipt is skipped rather than blanking the page", async ({ page }) => {
  await say(page, "RECEIPT step {not json");
  await say(page, step({ label: "Read Sheet1!A1", tool: "read", app: "excel", status: "done", detail: "ok" }));
  expect(await rows(page)).toHaveLength(1);
  // And nothing threw: the poller keeps running after a bad line.
  const alive = await page.evaluate(() => typeof window.live.step === "function");
  expect(alive).toBe(true);
});

test("a turn's work reads as one sentence of where it happened", async ({ page }) => {
  await say(page, "RECEIPT say model=vendor/model-x open=2");
  await say(page, step({ label: "Read data!A1:H6", tool: "read", app: "excel", status: "done", detail: "6x8" }));
  await say(page, step({ label: "Created slide 1", tool: "struct", app: "ppt", status: "done", detail: "s1" }));
  await say(page, step({ label: "Tried to add Scorecard", tool: "struct", app: "excel", status: "refused", detail: "exists" }));

  // While it runs, the card says so, in the present tense, with the apps.
  const card = page.locator(".acts").last();
  await expect(card).toHaveAttribute("data-mode", "live");
  await expect(card.locator(".acts-head .sum")).toHaveText("Working in Excel and PowerPoint");
  await expect(card.locator(".acts-head .pill.warn")).toHaveText("1 refused");

  // Each row carries its application, so Excel rows look like Excel.
  const tiles = await card.locator(".act .tile").evaluateAll((els) => els.map((e) => e.dataset.k));
  expect(tiles).toEqual(["excel", "ppt", "excel"]);

  // When the answer lands, it settles into the past tense and counts.
  await say(page, "ANSWER Done.");
  await expect(card).toHaveAttribute("data-mode", "done");
  await expect(card.locator(".acts-head .sum")).toHaveText("Worked in Excel and PowerPoint");
  await expect(card.locator(".acts-head .meta")).toContainText("3 steps");
});

test("a turn that touched nothing leaves no empty card behind", async ({ page }) => {
  await say(page, "RECEIPT say model=vendor/model-x open=0");
  await expect(page.locator(".acts")).toHaveCount(1);
  await say(page, "ANSWER Nothing to do: the sheet is already sorted.");
  await expect(page.locator(".acts")).toHaveCount(0);
  await expect(page.locator(".bot")).toContainText("already sorted");
});

test("a long finished run folds to its sentence, and opens again", async ({ page }) => {
  const calls = Array.from({ length: 12 }, (_, i) => ({
    id: `c${i}`,
    function: { name: "read", arguments: JSON.stringify({ handle: "excel:p.xlsx:S1", selector: `A${i}` }) },
  }));
  const labels = Object.fromEntries(calls.map((c, i) => [c.id, { text: `Read A${i}`, status: "done", app: "excel" }]));
  await page.evaluate(
    ([calls, labels]) =>
      window.live.render(
        [
          { role: "user", text: "read everything" },
          { role: "calls", text: JSON.stringify(calls) },
          { role: "assistant", text: "Read it all." },
        ],
        labels,
      ),
    [calls, labels],
  );
  const card = page.locator(".acts");
  await expect(card).toHaveAttribute("data-open", "0");
  await expect(page.locator(".act").first()).toBeHidden();
  await expect(card.locator(".acts-head .meta")).toContainText("12 steps");

  await card.locator(".acts-head").click();
  await expect(card).toHaveAttribute("data-open", "1");
  await expect(page.locator(".act").first()).toBeVisible();
});

test("a saved call opens to what it returned and what it asked for", async ({ page }) => {
  await page.evaluate(() =>
    window.live.render(
      [
        { role: "user", text: "read it" },
        {
          role: "calls",
          text: JSON.stringify([
            { id: "a", function: { name: "read", arguments: '{"handle":"excel:p.xlsx:S1","selector":"A1:B2"}' } },
          ]),
        },
        { role: "tool", id: "a", text: "grid S1: 2x2" },
      ],
      { a: { text: "Read A1:B2", status: "done", app: "excel" } },
    ),
  );
  await page.locator(".act").first().click();
  const body = page.locator(".body").first();
  await expect(body).toContainText("grid S1: 2x2");
  // Indented, not one long line of JSON.
  await expect(body).toContainText('"selector": "A1:B2"');
});
