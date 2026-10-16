// Putting Syn beside Excel, Word or PowerPoint, from the status menu.
//
// Offline. The console moves windows, and only on Windows with Syn in a
// window of its own, so the page's half of it is tested against an endpoint
// this spec answers itself: what the menu offers, what it sends, and what it
// shows of the answer. Moving real windows is `scripts/live-layout.ps1`.

import { expect, test } from "@playwright/test";

import { startConsole } from "./console.mjs";

let own;

test.beforeAll(async () => {
  own = await startConsole({ port: Number(process.env.SYN_UI_LAYOUT_PORT ?? 7825) });
});

test.afterAll(() => own?.stop());

/**
 * Answer /layout the way the console does: it keeps the choice, so a GET
 * gives back what a POST changed. Records every POST body.
 */
async function wire(page, { state = {}, reply } = {}) {
  const kept = { can: true, auto: true, side: "right", share: 0.3333, ...state };
  const asked = [];
  await page.route("**/layout", async (route) => {
    const r = route.request();
    if (r.method() === "GET") return route.fulfill({ json: kept });
    const body = JSON.parse(r.postData());
    asked.push(body);
    if (reply) return route.fulfill(reply);
    if (body.side) Object.assign(kept, { side: body.side, share: body.share });
    if ("auto" in body) kept.auto = body.auto;
    return route.fulfill({ json: { text: `Syn is on the ${kept.side} with ${Math.round(kept.share * 100)}% of the screen, and Excel has the rest.` } });
  });
  return asked;
}

async function openMenu(page) {
  await page.goto(own.url);
  await page.waitForFunction(() => !!window.live);
  await page.click("#hands");
  await expect(page.locator("#pop")).toHaveClass(/on/);
}

const pressed = (page, group) => page.locator(`#${group} [aria-pressed="true"]`);

test("a console that cannot move Syn does not offer the section", async ({ page }) => {
  await wire(page, { state: { can: false } });
  await openMenu(page);
  await expect(page.locator("#layout")).toBeHidden();
});

test("the real endpoint says whether Syn can be moved and what it remembers", async ({ page }) => {
  // Not the answers themselves: whether Syn can be moved depends on the
  // machine (yes on Windows with Syn open, no in a sandbox). That each is
  // there, and of the right kind, is what the page relies on.
  const r = await page.request.get(own.url.replace(/\/$/, "") + "/layout");
  expect(r.ok()).toBe(true);
  const s = await r.json();
  expect(typeof s.can).toBe("boolean");
  expect(typeof s.auto).toBe("boolean");
  expect(["left", "right"]).toContain(s.side);
  expect(s.share).toBeGreaterThanOrEqual(0.2);
  expect(s.share).toBeLessThanOrEqual(0.8);
});

test("the menu shows what the console remembers, and by default Syn arranges by itself", async ({ page }) => {
  await wire(page);
  await openMenu(page);
  await expect(page.locator("#layout")).toBeVisible();
  await expect(page.locator("#layAuto button")).toHaveText(["On", "Off"]);
  await expect(page.locator("#laySide button")).toHaveText(["Left", "Right"]);
  await expect(page.locator("#layShare button")).toHaveText(["25%", "33%", "40%", "50%", "67%"]);
  await expect(pressed(page, "layAuto")).toHaveText("On");
  await expect(pressed(page, "laySide")).toHaveText("Right");
  await expect(pressed(page, "layShare")).toHaveText("33%");
  await expect(page.locator("#layHint")).toContainText("take theirs when Syn opens them");
});

test("a choice the person made earlier is what the menu shows", async ({ page }) => {
  await wire(page, { state: { auto: false, side: "left", share: 0.5 } });
  await openMenu(page);
  await expect(pressed(page, "layAuto")).toHaveText("Off");
  await expect(pressed(page, "laySide")).toHaveText("Left");
  await expect(pressed(page, "layShare")).toHaveText("50%");
  await expect(page.locator("#layHint")).toContainText("side by side, edge to edge");
});

test("a click on a side or a share arranges at once, with the other one it is combined with", async ({ page }) => {
  const asked = await wire(page);
  await openMenu(page);

  await page.click('#layShare [data-share="0.5"]');
  await expect(page.locator("#layHint")).toContainText("Excel has the rest");
  // The choice and nothing about automatic: that is a setting of its own.
  expect(asked.at(-1)).toEqual({ side: "right", share: 0.5 });

  await page.click('#laySide [data-side="left"]');
  await expect.poll(() => asked.length).toBe(2);
  // The share chosen a moment ago is kept: a side is not a reset.
  expect(asked.at(-1)).toEqual({ side: "left", share: 0.5 });
  await expect(pressed(page, "laySide")).toHaveText("Left");
  await expect(pressed(page, "layShare")).toHaveText("50%");

  // A third is a third: 0.3333 of 1920 is 640, where 0.33 is 634.
  await page.click('#layShare [data-share="0.3333"]');
  await expect.poll(() => asked.length).toBe(3);
  expect(asked.at(-1)).toEqual({ side: "left", share: 0.3333 });
});

test("the switch turns arranging by itself off and on, and says so", async ({ page }) => {
  const asked = await wire(page);
  await openMenu(page);

  await page.click('#layAuto [data-auto="false"]');
  await expect(pressed(page, "layAuto")).toHaveText("Off");
  expect(asked.at(-1)).toEqual({ auto: false });

  await page.click('#layAuto [data-auto="true"]');
  await expect(pressed(page, "layAuto")).toHaveText("On");
  expect(asked.at(-1)).toEqual({ auto: true });
  expect(asked.length).toBe(2);
});

test("the choice is the console's, so it is there again when the page is opened afresh", async ({ page }) => {
  const asked = await wire(page);
  await openMenu(page);
  await page.click('#laySide [data-side="left"]');
  await page.click('#layShare [data-share="0.25"]');
  await page.click('#layAuto [data-auto="false"]');
  await expect.poll(() => asked.length).toBe(3);

  await page.reload();
  await page.waitForFunction(() => !!window.live);
  await page.click("#hands");
  await expect(pressed(page, "laySide")).toHaveText("Left");
  await expect(pressed(page, "layShare")).toHaveText("25%");
  await expect(pressed(page, "layAuto")).toHaveText("Off");
  // Remembered, not re-applied: opening the menu is not a request.
  expect(asked.length).toBe(3);
});

test("what the console says when it cannot arrange is shown as it is, and the choice stays unchanged on screen", async ({ page }) => {
  await wire(page, {
    reply: { status: 409, contentType: "text/plain", body: "Syn's own window is not on screen: open Syn from its shortcut and try again." },
  });
  await openMenu(page);
  await page.click('#layShare [data-share="0.4"]');
  await expect(page.locator("#layHint")).toContainText("open Syn from its shortcut");
  // The console refused, so the button the person pressed is not shown as the layout.
  await expect(pressed(page, "layShare")).toHaveText("33%");
});
