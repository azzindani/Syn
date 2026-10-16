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

/** Answer /layout the way the console would, and record what was asked. */
async function wire(page, { can = true, reply } = {}) {
  const asked = [];
  await page.route("**/layout", async (route) => {
    const r = route.request();
    if (r.method() === "GET") return route.fulfill({ json: { can } });
    asked.push(JSON.parse(r.postData()));
    return reply ? route.fulfill(reply) : route.fulfill({ json: { text: "Syn is on the right with 33% of the screen, and Excel has the rest." } });
  });
  return asked;
}

async function openMenu(page) {
  await page.goto(own.url);
  await page.waitForFunction(() => !!window.live);
  await page.click("#hands");
  await expect(page.locator("#pop")).toHaveClass(/on/);
}

test("a console that cannot move Syn does not offer the section", async ({ page }) => {
  await wire(page, { can: false });
  await openMenu(page);
  await expect(page.locator("#layout")).toBeHidden();
});

test("the real endpoint answers whether Syn can be moved, as a plain yes or no", async ({ page }) => {
  // Not the answer itself: on a Windows machine with Syn open it is yes, in
  // a sandbox it is no. That it is one or the other is what the page relies on.
  const r = await page.request.get(own.url.replace(/\/$/, "") + "/layout");
  expect(r.ok()).toBe(true);
  expect(typeof (await r.json()).can).toBe("boolean");
});

test("the menu offers a side and a share when Syn can be moved", async ({ page }) => {
  await wire(page);
  await openMenu(page);
  await expect(page.locator("#layout")).toBeVisible();
  await expect(page.locator("#laySide button")).toHaveText(["Left", "Right"]);
  await expect(page.locator("#layShare button")).toHaveText(["25%", "33%", "40%", "50%", "67%"]);
  // Nothing has been chosen, so what is shown is the default: right, a third.
  await expect(page.locator('#laySide [aria-pressed="true"]')).toHaveText("Right");
  await expect(page.locator('#layShare [aria-pressed="true"]')).toHaveText("33%");
});

test("a click arranges at once, with the side and the share it is combined with", async ({ page }) => {
  const asked = await wire(page);
  await openMenu(page);

  await page.click('#layShare [data-share="0.5"]');
  await expect(page.locator("#layHint")).toContainText("Excel has the rest");
  expect(asked.at(-1)).toEqual({ side: "right", share: 0.5 });

  await page.click('#laySide [data-side="left"]');
  await expect.poll(() => asked.length).toBe(2);
  // The share chosen a moment ago is kept: a side is not a reset.
  expect(asked.at(-1)).toEqual({ side: "left", share: 0.5 });
  await expect(page.locator('#laySide [aria-pressed="true"]')).toHaveText("Left");
  await expect(page.locator('#layShare [aria-pressed="true"]')).toHaveText("50%");

  // A third is a third: 0.3333 of 1920 is 640, where 0.33 is 634.
  await page.click('#layShare [data-share="0.3333"]');
  await expect.poll(() => asked.length).toBe(3);
  expect(asked.at(-1)).toEqual({ side: "left", share: 0.3333 });
});

test("the last choice comes back the next time the page opens, without moving anything", async ({ page }) => {
  const asked = await wire(page);
  await openMenu(page);
  await page.click('#laySide [data-side="left"]');
  await page.click('#layShare [data-share="0.25"]');
  await expect.poll(() => asked.length).toBe(2);

  await page.reload();
  await page.waitForFunction(() => !!window.live);
  await page.click("#hands");
  await expect(page.locator('#laySide [aria-pressed="true"]')).toHaveText("Left");
  await expect(page.locator('#layShare [aria-pressed="true"]')).toHaveText("25%");
  // Remembered, not re-applied: opening the menu is not a request.
  expect(asked.length).toBe(2);
});

test("what the console says when it cannot arrange is shown as it is", async ({ page }) => {
  await wire(page, {
    reply: { status: 409, contentType: "text/plain", body: "Syn's own window is not on screen: open Syn from its shortcut and try again." },
  });
  await openMenu(page);
  await page.click('#layShare [data-share="0.4"]');
  await expect(page.locator("#layHint")).toContainText("open Syn from its shortcut");
});
