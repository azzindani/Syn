// Two runs at once: an MCP client working while another one does, or a
// terminal run beside the console. The console followed whichever log was
// written last, so with two runs taking turns it flipped on every step,
// and every flip wiped the view and replayed the other run from the top.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { expect, test } from "@playwright/test";

import { startConsole } from "./console.mjs";

let url, stop, live;
const banner = "RECEIPT env ok\nRECEIPT session s\nRECEIPT registry 0\n";
const step = (label) => "RECEIPT step " + JSON.stringify({ label, tool: "read", app: "excel", status: "done", detail: "1x1" }) + "\n";
const put = (name, text) => fs.appendFileSync(path.join(live, name), text);

test.beforeAll(async () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "syn-concurrent-"));
  live = path.join(home, "live");
  fs.mkdirSync(live, { recursive: true });
  // Run A, already going when the console opens.
  put("100.log", banner + step("Read A1"));
  ({ url, stop } = await startConsole({ port: Number(process.env.SYN_UI_CONCURRENT_PORT ?? 7817), env: { AGENT_HOME: home } }));
});

test.afterAll(() => stop?.());

test("two runs taking turns do not steal the view from each other", async ({ page }) => {
  await page.goto(url);
  await page.waitForFunction(() => window.live && window.live.sse === true, null, { timeout: 10_000 });
  // The page adopts whatever is newest on load; make that run A.
  put("100.log", step("Read A2"));
  await expect(page.locator(".act .nm").filter({ hasText: "Read A2" })).toHaveCount(1);
  await expect.poll(() => page.evaluate(() => window.live.source)).toBe("100.log");

  // Run B starts, and the two take turns.
  // Checked right after each of B's writes: the old console ended up back
  // on A too, having wiped and replayed it on the way.
  const seen = [];
  for (let i = 0; i < 3; i++) {
    put("200.log", (i === 0 ? banner : "") + step(`Read B${i}`));
    await page.waitForTimeout(300);
    seen.push(await page.evaluate(() => window.live.source));
    put("100.log", step(`Read A${i + 3}`));
    await page.waitForTimeout(300);
  }
  expect(seen).toEqual(["100.log", "100.log", "100.log"]);
  await expect(page.locator(".act .nm").filter({ hasText: "Read A5" })).toHaveCount(1);
  expect(await page.evaluate(() => window.live.source)).toBe("100.log");
  const names = await page.locator(".act .nm").allTextContents();
  expect(names.filter((n) => n.startsWith("Read B"))).toEqual([]);
  // Nothing was wiped and replayed: A's rows are each there once.
  expect(names.filter((n) => n === "Read A3")).toHaveLength(1);
});
