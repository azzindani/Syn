// The parts of t3code's shell that a long run needs, checked.
//
// docs/design/DIGEST-08 named five things beyond the tool rows: the three-layer
// contrast tokens (section 8), the timeline minimap (section 5), the
// banner stack with activity outranking error (section 6), the budget
// meter (section 7, since taken out of the page: the check below now
// holds it out), and long messages folding away (section 9). Writing the digest is not building them, and this is
// what says which of the two happened.
//
// Its own console, pinned to a log it owns: any other console on the
// machine would otherwise become the newest run and repaint the page
// mid-assertion.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { test, expect } from "@playwright/test";

import { startConsole } from "./console.mjs";

let own;
let url;

test.beforeAll(async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "syn-shell-"));
  const log = path.join(dir, "quiet.log");
  fs.writeFileSync(log, "");
  own = await startConsole({ port: Number(process.env.SYN_SHELL_TEST_PORT ?? 7807), tail: log });
  url = own.url;
});

test.afterAll(() => own?.stop());

test.beforeEach(async ({ page }) => {
  await page.goto(url);
  await page.waitForFunction(() => !!window.live);
  await page.waitForFunction(() => window.live.source !== null || window.live.sse, null, { timeout: 15_000 });
  await page.evaluate(() => window.live.quiet());
});

test("one dial moves every colour, and no component knows about it", async ({ page }) => {
  const read = () =>
    page.evaluate(() => {
      const s = getComputedStyle(document.documentElement);
      return {
        fg: s.getPropertyValue("--foreground").trim(),
        muted: s.getPropertyValue("--muted-foreground").trim(),
        border: s.getPropertyValue("--border").trim(),
        raw: s.getPropertyValue("--raw-foreground").trim(),
      };
    });

  const base = await read();
  await page.evaluate(() => window.live.contrast(160));
  const high = await read();
  await page.evaluate(() => window.live.contrast(60));
  const low = await read();

  // The theme colour never changes; the derived ones do.
  expect(high.raw).toBe(base.raw);
  expect(low.raw).toBe(base.raw);
  expect(high.fg).not.toBe(base.fg);
  expect(low.fg).not.toBe(base.fg);
  expect(high.muted).not.toBe(base.muted);
  expect(high.border).not.toBe(base.border);

  await page.evaluate(() => window.live.contrast(100));
});

test("the contrast setting survives a reload", async ({ page }) => {
  await page.evaluate(() => window.live.contrast(140));
  await page.reload();
  await page.waitForFunction(() => !!window.live);
  const boost = await page.evaluate(() =>
    getComputedStyle(document.documentElement).getPropertyValue("--contrast-boost").trim(),
  );
  expect(boost).toBe("40%");
  await page.evaluate(() => window.live.contrast(100));
});

test("activity outranks the error behind it", async ({ page }) => {
  await page.evaluate(() => {
    window.live.banner("a", { variant: "error", priority: "urgent", title: "The run stopped" });
    window.live.banner("b", { variant: "info", title: "A notice" });
    window.live.banner("c", { variant: "activity", priority: "activity", title: "Working" });
  });

  // Only the front one shows; the rest collapse behind a count.
  let shown = await page.evaluate(() => window.live.banners);
  expect(shown).toHaveLength(1);
  expect(shown[0].title).toBe("Working");
  await expect(page.locator("#peek")).toHaveText("2 more");

  await page.locator("#peek").click();
  shown = await page.evaluate(() => window.live.banners);
  expect(shown.map((b) => b.variant)).toEqual(["activity", "error", "info"]);
});

test("a banner can be dismissed, and the activity one cannot", async ({ page }) => {
  await page.evaluate(() => {
    window.live.banner("x", { variant: "warning", title: "That model was busy" });
    window.live.banner("y", { variant: "activity", priority: "activity", title: "Working", dismiss: false });
  });
  await expect(page.locator('.bn[data-variant="activity"] .xx')).toHaveCount(0);

  await page.locator("#peek").click();
  await page.locator('.bn[data-variant="warning"] .xx').click();
  const shown = await page.evaluate(() => window.live.banners);
  expect(shown.map((b) => b.title)).toEqual(["Working"]);
});

test("a live run breathes in the thread, below its work, until it answers", async ({ page }) => {
  // Between steps the thread showed nothing, and a run started from
  // elsewhere (a terminal, an MCP client) never lit the status pill either.
  const say = (l) => page.evaluate((x) => window.live.step(x), l);
  const breath = page.locator("#tl .breath");
  const pill = page.locator("#hands .dots");
  await say("RECEIPT say model=vendor/x open=0");
  await expect(breath).toBeVisible();
  await expect(breath.locator("i")).toHaveCount(3);
  await expect(pill).toBeVisible();

  await say('RECEIPT step {"label":"Read data!A1","tool":"read","app":"excel","status":"done","detail":"x"}');
  const last = () => page.evaluate(() => {
    const b = document.querySelector("#tl .breath");
    return !!b && b === b.parentElement.lastElementChild;
  });
  expect(await last()).toBe(true);

  // While the reply is being written, the words are the sign of life.
  await say('RECEIPT delta {"kind":"text","text":"Here is"}');
  await expect(breath).toBeHidden();

  await say("ANSWER Here is the answer.");
  await expect(breath).toHaveCount(0);
  await expect(pill).toBeHidden();
});

test("the first run of a new conversation breathes in the sidebar before the chat is saved", async ({ page }) => {
  // A chat is listed once its first turn is saved, so the first run of a
  // new conversation had no row to light: the live check found none.
  const say = (l) => page.evaluate((x) => window.live.step(x), l);
  await say("RECEIPT say model=vendor/x open=0 chat=c1-notsavedyet");
  const row = page.locator('#chats .chat[data-id="c1-notsavedyet"]');
  await expect(row).toHaveClass(/pending/);
  await expect(row).toHaveClass(/working/);
  await expect(row.locator(".run")).toBeVisible();
  await say("ANSWER done");
  // The real list comes back, and a chat that was never saved is not in it.
  await expect(row).toHaveCount(0);
});

test("the thread a run is in breathes in the sidebar, and only that one", async ({ page }) => {
  // The chat list is the shared one in .agent/chats; the run names its chat.
  const rows = page.locator("#chats .chat");
  test.skip((await rows.count()) < 2, "needs two saved chats to tell them apart");
  const id = await rows.nth(1).getAttribute("data-id");
  const say = (l) => page.evaluate((x) => window.live.step(x), l);
  await say(`RECEIPT say model=vendor/x open=0 chat=${id}`);
  await expect(page.locator(`#chats .chat[data-id="${id}"] .run`)).toBeVisible();
  await expect(page.locator("#chats .chat.working")).toHaveCount(1);
  await expect(page.locator(`#chats .chat[data-id="${id}"] .run i`)).toHaveCount(3);
  // Seen live: a stray comment-closer broke the rule that hides the rest,
  // and every thread in the list breathed.
  await expect(page.locator("#chats .chat:not(.working) .run").first()).toBeHidden();
  await say("ANSWER done");
  await expect(page.locator("#chats .chat.working")).toHaveCount(0);
});

test("the run's lifecycle drives the banners without being told twice", async ({ page }) => {
  const say = (l) => page.evaluate((x) => window.live.step(x), l);
  await say("RECEIPT say model=qwen/qwen3-coder");
  // No "Working on <model>" banner: asked for its removal, because the dots
  // and the work card already say it. A banner is kept for what needs reading.
  expect(await page.evaluate(() => window.live.banners)).toHaveLength(0);

  await say("RECEIPT retry model=z-ai/glm-4.6");
  let shown = await page.evaluate(() => window.live.banners);
  expect(shown.map((b) => b.variant)).toEqual(["warning"]);

  await say("ANSWER done");
  shown = await page.evaluate(() => window.live.banners);
  expect(shown.every((b) => b.variant !== "activity")).toBe(true);
});

test("while a run is going the send button is Stop, and it goes back after", async ({ page }) => {
  // A stuck turn could only be left by closing the page: "Stop everything"
  // queued behind the very turn it was meant to stop.
  const say = (l) => page.evaluate((x) => window.live.step(x), l);
  const btn = page.locator("#send");
  await expect(btn).toHaveAttribute("aria-label", "Send");
  await say("RECEIPT say model=vendor/x open=0");
  await expect(btn).toHaveAttribute("aria-label", "Stop");
  await expect(btn).toBeEnabled();
  await expect(page.locator("#hint")).not.toContainText("working");
  await say("ANSWER done");
  await expect(btn).toHaveAttribute("aria-label", "Send");
});

test("the step budget stays with the model and is not counted down beside the composer", async ({ page }) => {
  // "37 steps left" next to the send button read as a countdown on the
  // person's own turn. The loop keeps its budget; the page says nothing.
  const send = await page.locator("#send").boundingBox();
  await page.evaluate(() => window.live.step('RECEIPT budget {"step":3,"max":40}'));
  await expect(page.locator("#meter")).toHaveCount(0);
  await expect(page.getByText(/steps? left/)).toHaveCount(0);
  const after = await page.locator("#send").boundingBox();
  expect(Math.abs(after.x - send.x)).toBeLessThan(1);
});

test("a long pasted message folds instead of pushing the run off the screen", async ({ page }) => {
  await page.evaluate(() => {
    const tl = document.querySelector("#tl");
    tl.textContent = "";
    const you = document.createElement("div");
    you.className = "you";
    you.textContent = Array.from({ length: 80 }, (_, i) => `log line ${i}`).join("\n");
    tl.appendChild(you);
    window.foldIfLong(you);
  });
  const you = page.locator(".you");
  await expect(you).toHaveClass(/tall/);
  const folded = (await you.boundingBox()).height;
  expect(folded).toBeLessThan(200);

  await page.locator(".morebtn").click();
  const opened = (await you.boundingBox()).height;
  expect(opened).toBeGreaterThan(folded * 2);
  await expect(page.locator(".morebtn")).toHaveText("Show less");
});

test("the minimap appears once there is more than one turn, and navigates", async ({ page }) => {
  // Below two turns it says nothing a scrollbar does not.
  await page.evaluate(() => window.live.render([{ role: "user", text: "only one" }], {}));
  expect(await page.evaluate(() => window.live.mapReady)).toBe(false);

  await page.evaluate(() =>
    window.live.render(
      [
        { role: "user", text: "build the scorecard" },
        { role: "assistant", text: "done" },
        { role: "user", text: "now the deck" },
        { role: "assistant", text: "done" },
        { role: "user", text: "and the memo" },
      ],
      {},
    ),
  );
  expect(await page.evaluate(() => window.live.turns)).toEqual([
    "build the scorecard",
    "now the deck",
    "and the memo",
  ]);
  expect(await page.evaluate(() => window.live.mapReady)).toBe(true);
  await expect(page.locator(".tick")).toHaveCount(3);

  // Keyboard: focus the strip, arrow down, and the preview follows.
  await page.locator("#mapstrip").focus();
  await expect(page.locator("#mappeek")).toHaveClass(/on/);
  await expect(page.locator("#mappeek .t")).toHaveText("build the scorecard");
  await page.keyboard.press("ArrowDown");
  await expect(page.locator("#mappeek .t")).toHaveText("now the deck");
  await page.keyboard.press("End");
  await expect(page.locator("#mappeek .t")).toHaveText("and the memo");
  await expect(page.locator(".tick.here")).toHaveCount(1);
});

test("the minimap tapers by distance, so the one under the cursor is findable", async ({ page }) => {
  await page.evaluate(() =>
    window.live.render(
      Array.from({ length: 6 }, (_, i) => ({ role: "user", text: `turn ${i}` })),
      {},
    ),
  );
  await page.locator("#mapstrip").focus();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  // The taper is a 150ms transition, so measuring on the next frame
  // reads the start value. Wait for it to settle rather than sleeping a
  // guessed amount.
  await expect
    .poll(async () => page.$eval('.tick[data-near="0"]', (e) => Math.round(e.getBoundingClientRect().width)))
    .toBe(24);
  const widths = await page.$$eval(".tick", (els) => els.map((e) => e.getBoundingClientRect().width));
  // Index 2 is the active one; its neighbours are narrower, and the far
  // ones narrower still.
  expect(widths[2]).toBeGreaterThan(widths[1]);
  expect(widths[1]).toBeGreaterThan(widths[0]);
  expect(widths[5]).toBeLessThan(widths[3]);
});

test("a summarised history shows its seam rather than losing the turns silently", async ({ page }) => {
  await page.evaluate(() =>
    window.live.render(
      [
        { role: "user", text: "What have we done so far?" },
        { role: "assistant", text: "## Objective\n- build the scorecard" },
        { role: "user", text: "carry on" },
      ],
      {},
    ),
  );
  await expect(page.locator(".seam")).toHaveText("earlier turns summarised");
  // The question is the summariser's, not the human's: it must not appear
  // as something the person said, or as a turn on the minimap.
  expect(await page.evaluate(() => window.live.turns)).toEqual(["carry on"]);
  await expect(page.locator(".you")).toHaveCount(1);
});

test("the approval box can be read before it is answered", async ({ page }) => {
  // `shell` always stops for a human, so this is the most important
  // widget on the page. A long command must be scrollable, and reachable
  // by keyboard: someone deciding whether to run it has to be able to
  // see all of it first.
  await page.evaluate(() => {
    window.showAsk({
      program: "robocopy",
      preview: "robocopy " + Array.from({ length: 40 }, (_, i) => `--flag-${i}`).join(" "),
      why: "to mirror the report folder",
    });
  });
  const box = page.locator("#askcmd");
  await expect(box).toBeVisible();
  expect(await box.getAttribute("tabindex")).toBe("0");

  // It scrolls rather than reflowing: a re-wrapped command is a
  // different command than the one that will run.
  const over = await box.evaluate((e) => e.scrollWidth > e.clientWidth + 1);
  expect(over).toBe(true);

  await box.focus();
  expect(await page.evaluate(() => document.activeElement.id)).toBe("askcmd");

  // The model's stated reason is shown beside it, as a claim, not as an
  // instruction.
  await expect(page.locator("#askwhy")).toContainText("to mirror the report folder");
});

test("evidence: the shell with everything up at once", async ({ page }) => {
  const shots = path.join(process.cwd(), "..", "..", "testbed", "shots");
  fs.mkdirSync(shots, { recursive: true });

  await page.evaluate(() => {
    window.live.render(
      [
        { role: "user", text: "Build the quarterly scorecard and a deck from it." },
        { role: "assistant", text: "Reading the source data first." },
        { role: "user", text: "also add a chart" },
      ],
      {},
    );
    window.live.banner("busy", { variant: "activity", priority: "activity", title: "Working on qwen3-coder", dismiss: false });
    window.live.banner("retry", {
      variant: "warning",
      priority: "urgent",
      title: "That model was busy",
      description: "Retrying with z-ai/glm-4.6.",
    });
    for (const l of [
      'RECEIPT step {"label":"Read data!A1:H500","tool":"read","app":"excel","status":"done","detail":"grid data: 500x8"}',
      'RECEIPT step {"label":"Added Scorecard","tool":"struct","app":"excel","status":"done","detail":"sheet added"}',
      'RECEIPT step {"label":"Refused to add Scorecard","tool":"struct","app":"excel","status":"refused","detail":"already exists"}',
      'RECEIPT step {"label":"Drew a chart on Dashboard!A1:H16","tool":"struct","app":"excel","status":"done","detail":"column chart"}',
    ]) window.live.step(l);
  });

  await expect(page.locator(".act")).toHaveCount(4);
  await expect(page.locator("#peek")).toBeVisible();
  await page.locator("#mapstrip").hover();
  await page.screenshot({ path: path.join(shots, "shell-full.png") });
});

test("the theme can be pinned, and 'system' follows the machine", async ({ page }) => {
  const theme = () => page.evaluate(() => document.documentElement.dataset.theme);

  await page.emulateMedia({ colorScheme: "dark" });
  await page.evaluate(() => window.live.theme("system"));
  expect(await theme()).toBe("dark");
  await page.emulateMedia({ colorScheme: "light" });
  await expect.poll(theme).toBe("light");

  // Pinned, it no longer follows, and it survives a reload.
  await page.evaluate(() => window.live.theme("dark"));
  await page.emulateMedia({ colorScheme: "light" });
  expect(await theme()).toBe("dark");
  await page.reload();
  await page.waitForFunction(() => !!window.live);
  expect(await theme()).toBe("dark");
  await expect(page.locator('#theme button[data-pref="dark"]')).toHaveAttribute("aria-pressed", "true");

  await page.evaluate(() => window.live.theme("system"));
});

test("the first screen is the mark and the apps, nothing to read past", async ({ page }) => {
  await page.evaluate(() => window.live.render([], {}));
  await expect(page.locator(".empty .hero")).toBeVisible();
  await expect(page.locator(".empty .works")).toContainText("Excel");
  await expect(page.locator(".empty h2, .empty p, .empty button")).toHaveCount(0);
  await expect(page.locator("#box")).toHaveValue("");
});
