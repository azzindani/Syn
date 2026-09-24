// Does the thing actually work?
//
// Not "does the loop look plausible" — does a command entered in the
// console reach a document, change it, come back changed, and stop when a
// gate says stop. No model, no API key, no Office: `attach` without `live`
// binds a handle to the in-memory document model, which is tier 2 in
// docs/10-adding-tools-remotely.md and exercises ops, the Runner, the
// queue, the registry and every gate on the way.
//
// What this cannot tell you: whether the COM call is right, or whether
// real Excel ends up correct. That needs the desk and always will —
// `scripts/live-excel-smoke.ps1` is that test.
//
// Screenshots land in testbed/shots/ as evidence a person can look at
// without running anything.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { test, expect } from "@playwright/test";

import { readState, repo, startConsole } from "./console.mjs";

const { url } = readState();
const shots = path.join(repo, "testbed", "shots");

/** Run one console command the way the page's own controls do. */
async function cmd(page, line) {
  return page.evaluate((l) => window.cli(l), line);
}

async function shot(page, name) {
  fs.mkdirSync(shots, { recursive: true });
  await page.screenshot({ path: path.join(shots, `${name}.png`) });
}

// A handle of its own per test, so one test's document is never another's.
let n = 0;
const fresh = () => `t${Date.now().toString(36)}${n++}`;

test.describe("driving a document", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto(url);
    await page.waitForFunction(() => !!window.live);
    await page.waitForLoadState("networkidle");
  });

  test("a write reaches the document and reads back", async ({ page }) => {
    const file = fresh();
    const h = `excel:${file}:Sheet1`;

    expect(await cmd(page, `attach excel ${file} Sheet1`)).toContain(`RECEIPT attached=${h}`);

    const wrote = await cmd(page, `write ${h} Sheet1!A1:B1 hello|world`);
    expect(wrote).not.toContain("ERROR");

    // The claim is not "the write was accepted", it is "the document says
    // so afterwards".
    const got = await cmd(page, `read ${h} Sheet1!A1:B1`);
    expect(got).toContain("hello");
    expect(got).toContain("world");
  });

  test("a single cell is a selector, the way the tool's own example says", async ({ page }) => {
    // `write{"selector":"Sheet1!G1"}` is the worked example on the `write`
    // tool. The document model used to refuse it for want of a colon, so
    // the schema promised something only the live path delivered.
    const file = fresh();
    const h = `excel:${file}:Sheet1`;
    await cmd(page, `attach excel ${file} Sheet1`);
    expect(await cmd(page, `write ${h} Sheet1!G1 one`)).not.toContain("ERROR");
    expect(await cmd(page, `read ${h} Sheet1!G1`)).toContain("one");
  });

  test("one write fills a block, which is the whole point of the grid encoding", async ({ page }) => {
    // The capability claim that separates this from a macro recorder: a
    // column is one call, not eleven.
    const file = fresh();
    const h = `excel:${file}:Sheet1`;
    await cmd(page, `attach excel ${file} Sheet1`);
    const out = await cmd(page, `write ${h} Sheet1!A1:B3 a1|b1;a2|b2;a3|b3`);
    expect(out).not.toContain("ERROR");
    const got = await cmd(page, `read ${h} Sheet1!A1:B3`);
    for (const v of ["a1", "b1", "a2", "b2", "a3", "b3"]) expect(got).toContain(v);
  });

  test("a formula keeps its commas instead of being cut in half", async ({ page }) => {
    // The comma-separator bug cost a capability run thirty-two formulas:
    // `=COUNTIF(A:A,x)` was split into two cells. The separator is a pipe
    // now, and this is the regression.
    const file = fresh();
    const h = `excel:${file}:Sheet1`;
    await cmd(page, `attach excel ${file} Sheet1`);
    expect(await cmd(page, `write ${h} Sheet1!C1 =COUNTIF(A:A,7)`)).not.toContain("ERROR");
    const got = await cmd(page, `read ${h} Sheet1!C1`);
    expect(got).toContain("=COUNTIF(A:A,7)");
  });

  test("an unknown handle is refused, not invented", async ({ page }) => {
    const out = await cmd(page, "read excel:nothing-here:Sheet1 Sheet1!A1:B1");
    expect(out.toLowerCase()).toMatch(/not open|unknown|registry/);
  });

  test("the console shows what is open", async ({ page }) => {
    const file = fresh();
    await cmd(page, `attach excel ${file} Sheet1`);
    expect(await cmd(page, "registry")).toContain(file);
  });
});

// The gates latch. `kill` stops dispatch for good — "fresh guard required"
// — which is the property worth having and exactly why these cannot run
// against the console every other spec shares. One `kill` used to take the
// rest of this file with it, and the damage looked like bugs in unrelated
// tests.
test.describe("the gates, each in a console of its own", () => {
  test("the kill switch stops a write that would otherwise land", async ({ page }) => {
    const own = await startConsole({ port: Number(process.env.SYN_GATE_TEST_PORT ?? 7803) });
    try {
      await page.goto(own.url);
      await page.waitForFunction(() => !!window.live);
      const file = fresh();
      const h = `excel:${file}:Sheet1`;
      await cmd(page, `attach excel ${file} Sheet1`);
      expect(await cmd(page, `write ${h} Sheet1!A1:B1 before|x`)).not.toContain("ERROR");
      expect(await cmd(page, `read ${h} Sheet1!A1:B1`)).toContain("before");

      await cmd(page, "kill");
      expect(await cmd(page, `write ${h} Sheet1!A1:B1 after|y`)).toMatch(/kill switch latched/i);

      // And it latches hard: a read is refused too. A kill that stopped
      // only writes would leave a run reading a document it must not
      // touch, and "fresh guard required" is the stronger promise.
      expect(await cmd(page, `read ${h} Sheet1!A1:B1`)).toMatch(/kill switch latched/i);
    } finally {
      own.stop();
    }
  });

  test("the app allowlist is enforced per app", async ({ page }) => {
    const own = await startConsole({ port: Number(process.env.SYN_GATE2_TEST_PORT ?? 7804) });
    try {
      await page.goto(own.url);
      await page.waitForFunction(() => !!window.live);
      const file = fresh();
      const h = `excel:${file}:Sheet1`;
      await cmd(page, `attach excel ${file} Sheet1`);
      expect(await cmd(page, `write ${h} Sheet1!A1:B1 ok|x`)).not.toContain("ERROR");

      // Lock the allowlist to a different app; this handle is now refused.
      await cmd(page, "allow word");
      expect(await cmd(page, `write ${h} Sheet1!A1:B1 nope|y`).then((s) => s.toLowerCase())).toMatch(/allowlist/);

      // And the cell was not written.
      const got = await cmd(page, `read ${h} Sheet1!A1:B1`);
      expect(got.includes("ok") || /allowlist/i.test(got)).toBe(true);
      expect(got).not.toContain("nope");
    } finally {
      own.stop();
    }
  });
});

test("evidence: a run on the console", async ({ page }) => {
  // Not an assertion about pixels — a screenshot a person can look at to
  // decide whether this is worth using. Driven through the same rendering
  // path a real run uses.
  // Its own console, pinned to a log it owns: any other console on the
  // machine would otherwise become the "newest" run and this page would
  // follow that one instead, wiping the timeline mid-shot.
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "syn-shot-"));
  const log = path.join(dir, "quiet.log");
  fs.writeFileSync(log, "");
  const own = await startConsole({ port: Number(process.env.SYN_SHOT_TEST_PORT ?? 7806), tail: log });
  try {
  await page.goto(own.url);
  await page.waitForFunction(() => !!window.live);
  // `window.live` exists as soon as the script evaluates, but the page's
  // bootstrap is an async IIFE and opens the stream at the end of it.
  // Calling quiet() before that point closes nothing, and the stream then
  // starts and wipes the timeline mid-shot.
  await page.waitForFunction(() => window.live.source !== null || window.live.sse, null, { timeout: 15_000 });
  await page.evaluate(() => {
    window.live.quiet();
    document.querySelector("#tl").textContent = "";
  });

  const step = (o) => "RECEIPT step " + JSON.stringify(o);
  const lines = [
    step({ label: "Read data!A1:H500", tool: "read", app: "excel", status: "done", detail: "grid data: 500x8" }),
    step({ label: "Added Scorecard", tool: "struct", app: "excel", status: "done", detail: "sheet added" }),
    step({
      label: "Wrote Scorecard!B2:B12",
      tool: "write",
      app: "excel",
      status: "done",
      detail: "11 cells from one call, the formula stepping per row",
    }),
    step({
      label: "Refused to add Scorecard",
      tool: "struct",
      app: "excel",
      status: "refused",
      detail: "a sheet named Scorecard already exists",
    }),
    step({
      label: "Drew a chart on Dashboard!A1:H16",
      tool: "struct",
      app: "excel",
      status: "done",
      detail: "column chart, anchored to the range",
    }),
    step({ label: "Ran hostname", tool: "shell", app: "shell", status: "done", detail: "exit=0" }),
    "RECEIPT did " + JSON.stringify({ text: "Read 1 range, restructured 1 document, and ran 1 program" }),
  ];
  for (const l of lines) await page.evaluate((x) => window.live.step(x), l);

  await expect(page.locator(".act")).toHaveCount(6);
  await expect(page.locator(".did")).toContainText("Read 1 range");
  await shot(page, "console-run");

  // Expanded, which is how a person checks what a step actually did.
  await page.locator(".act").first().click();
  await expect(page.locator(".body").first()).toBeVisible();
  await shot(page, "console-run-expanded");

  await page.emulateMedia({ colorScheme: "light" });
  await shot(page, "console-run-light");
  } finally {
    own.stop();
  }
});
