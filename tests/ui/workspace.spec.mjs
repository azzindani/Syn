// The workspace picker: the folder a chat works in, chosen beside the model.
//
// The page can only ask the CLI (`dirs`, `workspace`); a browser will not
// hand it a folder's real path. What is checked here is that the asking and
// the showing line up: the folders offered are the CLI's, the one chosen is
// the one the CLI now holds, a bad path is refused in place, and
// "Everywhere" really lets go.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { expect, test } from "@playwright/test";

import { readState } from "./console.mjs";

const { url } = readState();

let root;

test.beforeAll(() => {
  root = fs.mkdtempSync(path.join(os.tmpdir(), "syn-ws-ui-"));
  fs.mkdirSync(path.join(root, "reports", "q3"), { recursive: true });
  fs.mkdirSync(path.join(root, "archive"));
});

test.afterAll(() => fs.rmSync(root, { recursive: true, force: true }));

async function cli(page, line) {
  const r = await page.request.post(url + "cmd", { headers: { Origin: url.replace(/\/$/, "") }, data: line });
  return (await r.json()).out;
}

test.beforeEach(async ({ page }) => {
  await page.goto(url);
  await expect(page.locator("#brand")).toContainText("Syn");
  await cli(page, "workspace off");
  await page.reload();
});

test.afterEach(async ({ page }) => {
  await cli(page, "workspace off");
});

test("a folder browsed to and used becomes the chat's workspace, in the CLI and on the pill", async ({ page }) => {
  await expect(page.locator("#wname")).toHaveText("Everywhere");
  await page.locator("#wpick").click();
  await expect(page.locator("#spaces")).toHaveClass(/on/);
  // The start page offers somewhere to begin and no folder to "use" yet.
  await expect(page.locator("#wlist .wrow").first()).toContainText("Everywhere");
  await expect(page.locator("#wuse")).toBeHidden();

  // Browsing is the CLI's `dirs`: paste the parent, then walk into it.
  await page.locator("#wpath").fill(root);
  await page.evaluate(p => window.eval(`browseSpace(${JSON.stringify(p)})`), root);
  await expect(page.locator("#wlist .wrow .wn")).toHaveText(["archive", "reports"]);
  await page.locator("#wlist .wrow", { hasText: "reports" }).click();
  await expect(page.locator("#wlist .crumb")).toContainText("reports");
  await expect(page.locator("#wlist .wrow .wn")).toHaveText(["q3"]);
  await page.locator("#wuse").click();

  await expect(page.locator("#spaces")).not.toHaveClass(/on/);
  await expect(page.locator("#wname")).toHaveText("reports");
  await expect(page.locator("#wpick")).toHaveClass(/set/);
  const held = JSON.parse((await cli(page, "workspace")).match(/RECEIPT workspace (\{.*\})/)[1]).path;
  expect(held.toLowerCase()).toBe(fs.realpathSync(path.join(root, "reports")).toLowerCase());
});

test("a pasted path that is not a folder is refused where it was typed", async ({ page }) => {
  await page.locator("#wpick").click();
  await page.locator("#wpath").fill(path.join(root, "no-such-folder"));
  await page.locator("#wpath").press("Enter");
  await expect(page.locator("#wnote")).toHaveClass(/err/);
  await expect(page.locator("#wnote")).toContainText("not a folder that exists");
  await expect(page.locator("#spaces")).toHaveClass(/on/);
  await expect(page.locator("#wname")).toHaveText("Everywhere");
});

test("a pasted folder is used, remembered as recent, and Everywhere lets go of it", async ({ page }) => {
  await page.locator("#wpick").click();
  await page.locator("#wpath").fill(root);
  await page.locator("#wpath").press("Enter");
  await expect(page.locator("#wname")).toHaveText(path.basename(root));

  await page.locator("#wpick").click();
  await expect(page.locator("#wlist .wh").first()).toHaveText(/recent/i);
  await expect(page.locator("#wlist .wrow", { hasText: path.basename(root) }).first()).toHaveAttribute("aria-selected", "true");
  await page.locator("#wlist .wrow", { hasText: "Everywhere" }).click();
  await expect(page.locator("#wname")).toHaveText("Everywhere");
  await expect(page.locator("#wpick")).not.toHaveClass(/set/);
  expect(await cli(page, "workspace")).toContain('"path":""');
});
