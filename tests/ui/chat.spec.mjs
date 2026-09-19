import { expect, test } from "@playwright/test";

import { readState } from "./console.mjs";

const { url } = readState();

// What the console calls a finished turn: the send button comes back.
async function turnSettled(page) {
  await expect(page.locator("#send")).toBeEnabled({ timeout: 150_000 });
}

test.beforeEach(async ({ page }) => {
  await page.goto(url);
  await expect(page.locator("#brand")).toContainText("Syn");
});

test("a typed prompt reaches the model and the reply lands in the thread", async ({ page }) => {
  await page.locator("#new").click();
  await expect(page.locator("#title")).toHaveText("New chat");

  const prompt = "Reply with exactly one word: pineapple";
  const box = page.locator("#box");
  await box.fill(prompt);

  // The spinner shares its line with the keyboard legend, so it is also the
  // check that hiding the legend did not hide the only sign of a running
  // turn. Start watching before the keystroke, or a fast model wins the race.
  const working = expect(page.locator("#hint .spin")).toBeVisible({ timeout: 20_000 });
  await box.press("Enter");
  await working;

  // The turn is painted at once rather than blanking the thread.
  await expect(page.locator("#tl .you").last()).toHaveText(prompt);
  await turnSettled(page);

  const failure = page.locator("#tl .fail");
  if (await failure.count()) {
    throw new Error(`the provider refused the turn: ${await failure.last().innerText()}`);
  }

  const reply = page.locator("#tl .bot").last();
  await expect(reply).toBeVisible();
  const text = (await reply.innerText()).trim();
  expect(text.length).toBeGreaterThan(0);
  console.log(`\n  prompt: ${prompt}\n  reply : ${text}\n`);

  // The thread now carries the turn, and the title took the first line.
  await expect(page.locator("#title")).toContainText("pineapple");
  await page.screenshot({ path: "../../testbed/playwright/reply.png", fullPage: false });
});

test("a reload lands back on the same thread", async ({ page }) => {
  // The hash is written once the thread list has loaded, which is a fetch
  // after first paint, so reading it straight off #brand races the boot.
  await page.waitForFunction(() => location.hash.length > 1);
  const id = await page.evaluate(() => location.hash.slice(1));
  expect(id).toMatch(/^c\d+-[0-9a-f]+$/);
  const title = await page.locator("#title").innerText();

  await page.reload();
  await expect(page.locator("#title")).toHaveText(title);
  expect(await page.evaluate(() => location.hash.slice(1))).toBe(id);
});

test("the machinery sits behind the status control, not in the sidebar", async ({ page }) => {
  // The sidebar's one job is threads: no hand chips, no kill switch.
  await expect(page.locator("#side #wire")).toHaveCount(0);
  await expect(page.locator("#side #killbtn")).toHaveCount(0);

  const pop = page.locator("#pop");
  await expect(pop).toBeHidden();
  await page.locator("#hands").click();
  await expect(pop).toBeVisible();
  await expect(pop.locator("#killbtn")).toBeVisible();
  await expect(pop.locator("#wire button").first()).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(pop).toBeHidden();
});
