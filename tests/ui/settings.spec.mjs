// Settings > API keys: how an installed copy, with no .env, gets a key.
//
// A console of its own, with its own data folder and an empty .env, so it
// starts with no key anywhere and the user's real keys are never read or
// written. The key used is fake; nothing here reaches a provider.
//
// What is checked: a first start says a key is needed and leads to
// Settings; a key saved there is kept out of every file but the keys file
// (and sealed in that one on Windows); the page is only ever told where a
// key comes from and its last four characters; and a refused paste is
// refused in its own row, not drawn a second time as a failure in the chat.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { expect, test } from "@playwright/test";

import { startConsole } from "./console.mjs";

const FAKE = "sk-test-FAKE-0000-settings-9z7q";
let url, stop, home;

test.beforeAll(async () => {
  home = fs.mkdtempSync(path.join(os.tmpdir(), "syn-settings-"));
  const empty = path.join(home, "empty.env");
  fs.writeFileSync(empty, "");
  ({ url, stop } = await startConsole({
    port: Number(process.env.SYN_UI_SETTINGS_PORT ?? 7821),
    env: {
      AGENT_HOME: home,
      AGENT_ENV_FILE: empty,
      // A key exported in the shell that runs the tests would count as one.
      AGENT_API_KEY: "",
      AGENT_API_KEY_OPENROUTER: "",
      AGENT_API_KEY_OPENCODE: "",
      AGENT_API_KEY_OPENCODE_GO: "",
      AGENT_AUTH_CONTENT: "",
    },
  }));
});

test.afterAll(() => {
  stop?.();
  fs.rmSync(home, { recursive: true, force: true });
});

const row = (page, name) => page.locator(`.krow[data-provider="${name}"]`);

/** Every file under the data folder whose text holds `needle`. */
function holding(dir, needle) {
  const out = [];
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) out.push(...holding(p, needle));
    else if (fs.readFileSync(p, "utf8").includes(needle)) out.push(path.relative(home, p));
  }
  return out;
}

test("a first start with no key says so, and its button opens Settings", async ({ page }) => {
  await page.goto(url);
  const banner = page.locator('#banners .bn[data-banner-id="nokey"]');
  await expect(banner).toContainText("Add an API key");
  await banner.locator(".ba").click();
  await expect(page.locator("#settings")).toHaveJSProperty("open", true);
  for (const name of ["openrouter", "opencode", "opencode-go"]) {
    await expect(row(page, name).locator(".kst")).toHaveText("No key yet");
  }
});

test("a saved key is sealed away, shown only by its last four, and removable", async ({ page }) => {
  await page.goto(url);
  await page.locator("#hands").click();
  await page.locator("#setbtn").click();
  const zen = row(page, "opencode");
  await expect(zen.locator(".kst")).toHaveText("No key yet");

  // Two things pasted at once is refused in the row, and only there.
  await zen.locator("input").fill("sk-one sk-two");
  await zen.locator("input").press("Enter");
  await expect(zen.locator(".kmsg.err")).toContainText("no spaces");
  await page.waitForTimeout(700);
  await expect(page.locator("#tl .fail")).toHaveCount(0);

  await zen.locator("input").fill(FAKE);
  await zen.locator(".btn.primary").click();
  await expect(zen.locator(".kst")).toHaveText("Saved · ends " + FAKE.slice(-4));
  await expect(zen.locator("input")).toHaveValue("");
  // With one key the first-start banner has nothing left to say.
  await expect(page.locator('#banners .bn[data-banner-id="nokey"]')).toHaveCount(0);

  // The key as typed is nowhere in the page, and in the data folder only
  // in the keys file, and not even there on Windows, where it is sealed.
  expect(await page.content()).not.toContain(FAKE);
  const where = holding(home, FAKE);
  if (process.platform === "win32") {
    expect(where).toEqual([]);
    expect(fs.readFileSync(path.join(home, "auth.json"), "utf8")).toContain("dpapi:");
  } else {
    expect(where).toEqual(["auth.json"]);
  }

  await zen.locator(".btn:not(.primary)").click();
  await expect(zen.locator(".kst")).toHaveText("No key yet");
  await expect(page.locator('#banners .bn[data-banner-id="nokey"]')).toHaveCount(1);
});
