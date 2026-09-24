import { expect, test } from "@playwright/test";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";

import { startConsole } from "./console.mjs";

// A provider of our own on loopback, speaking OpenRouter's /models shape
// and answering chat completions with one word. It records every request
// body, which is the point: the picker is only right if the model and the
// thinking level it shows are the ones the provider is actually sent.
const models = [
  {
    id: "vendor/thinker", name: "Vendor: Thinker", context_length: 200000,
    architecture: { input_modalities: ["text", "image"], output_modalities: ["text"] },
    pricing: { prompt: "0.000003", completion: "0.000015" },
    supported_parameters: ["tools", "reasoning"],
  },
  {
    id: "vendor/quick:free", name: "Vendor: Quick (free)", context_length: 32768,
    architecture: { output_modalities: ["text"] },
    pricing: { prompt: "0", completion: "0" },
    supported_parameters: ["tools"],
  },
  // Cannot call tools, so it must never be offered.
  { id: "vendor/chatty", name: "Vendor: Chatty", supported_parameters: ["temperature"] },
];
const bodies = [];
let server, provider, consoleUrl, stop, home;

test.beforeAll(async () => {
  server = http.createServer((req, res) => {
    let body = "";
    req.on("data", c => (body += c));
    req.on("end", () => {
      res.setHeader("Content-Type", "application/json");
      if (req.method === "GET" && req.url === "/api/v1/models") return res.end(JSON.stringify({ data: models }));
      if (req.method === "POST" && req.url === "/api/v1/chat/completions") {
        bodies.push(JSON.parse(body));
        return res.end(JSON.stringify({
          id: "r1", choices: [{ index: 0, finish_reason: "stop", message: { role: "assistant", content: "pineapple" } }],
        }));
      }
      res.statusCode = 404;
      res.end("{}");
    });
  });
  await new Promise(r => server.listen(0, "127.0.0.1", r));
  provider = `127.0.0.1:${server.address().port}`;
  home = fs.mkdtempSync(path.join(os.tmpdir(), "syn-models-"));
  ({ url: consoleUrl, stop } = await startConsole({
    port: Number(process.env.SYN_UI_MODELS_PORT ?? 7813),
    env: { AGENT_BASE_URL: `http://${provider}/api/v1`, AGENT_API_KEY: "test-key", AGENT_HOME: home },
  }));
});

test.afterAll(async () => {
  stop?.();
  await new Promise(r => server?.close(r));
});

async function openPicker(page) {
  await page.goto(consoleUrl);
  await expect(page.locator("#mname")).toHaveText(/./);
  await page.locator("#mpick").click();
  await expect(page.locator("#models")).toBeVisible();
}

test("the picker is the provider's own list, searchable, with what each model costs", async ({ page }) => {
  await openPicker(page);
  const rows = page.locator("#mlist .mrow");
  // Automatic first, then the two models that can call tools.
  await expect(rows).toHaveCount(3);
  await expect(rows.nth(0)).toContainText("Automatic");
  await expect(page.locator("#mlist")).not.toContainText("Chatty");
  const thinker = rows.filter({ hasText: "vendor/thinker" });
  await expect(thinker.locator(".mn")).toHaveText("Thinker");
  await expect(thinker).toContainText("thinks");
  await expect(thinker).toContainText("200K · $3 / $15");
  await expect(rows.filter({ hasText: "vendor/quick:free" })).toContainText("free");

  await page.locator("#msearch").fill("quick");
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText("vendor/quick:free");
  await expect(page.locator("#mfoot")).toContainText("2 models");
});

test("the model and thinking level picked are what the provider is sent", async ({ page }) => {
  await openPicker(page);
  await page.locator("#msearch").fill("thinker");
  await page.locator("#msearch").press("Enter");
  await expect(page.locator("#models")).toBeHidden();
  await expect(page.locator("#mname")).toHaveText("Thinker");
  await page.locator("#think").selectOption("high");

  const before = bodies.length;
  await page.locator("#new").click();
  await page.locator("#box").fill("Reply with one word");
  await page.locator("#box").press("Enter");
  await expect(page.locator("#tl .bot").last()).toContainText("pineapple", { timeout: 60_000 });
  const sent = bodies[bodies.length - 1];
  expect(bodies.length).toBeGreaterThan(before);
  expect(sent.model).toBe("vendor/thinker");
  expect(sent.reasoning).toEqual({ effort: "high" });
});

test("a fresh window shows what the console will use, and automatic goes back to the .env model", async ({ page }) => {
  // A new browser, with nothing remembered: what it shows has to come from
  // the console, which the previous test left on Thinker, thinking high.
  await page.goto(consoleUrl);
  await expect(page.locator("#mname")).toHaveText("Thinker");
  await expect(page.locator("#think")).toHaveValue("high");

  await page.locator("#mpick").click();
  await page.locator("#mlist .mrow.auto").click();
  await expect(page.locator("#mname")).toHaveText("Automatic");
  await page.locator("#think").selectOption("low");
  await page.locator("#new").click();
  await page.locator("#box").fill("Again");
  await page.locator("#box").press("Enter");
  await expect(page.locator("#tl .bot").last()).toContainText("pineapple", { timeout: 60_000 });
  const sent = bodies[bodies.length - 1];
  expect(sent.model).toBe("openrouter/auto");
  expect(sent.reasoning).toEqual({ effort: "low" });
});

test("the choice survives a reload", async ({ page }) => {
  await openPicker(page);
  await page.locator("#mlist .mrow", { hasText: "vendor/quick:free" }).click();
  await page.locator("#think").selectOption("medium");
  await page.reload();
  await expect(page.locator("#mname")).toHaveText("Quick (free)");
  await expect(page.locator("#think")).toHaveValue("medium");
});

test("a model the provider adds shows up on refresh, without a restart", async ({ page }) => {
  await openPicker(page);
  await expect(page.locator("#mlist")).not.toContainText("Brand New");
  models.push({ id: "vendor/brand-new", name: "Vendor: Brand New", supported_parameters: ["tools"] });
  await page.locator("#mrefresh").click();
  await expect(page.locator("#mlist")).toContainText("Brand New");
  await expect(page.locator("#mfoot")).toContainText("updated just now");
  await page.screenshot({ path: "../../testbed/playwright/model-picker.png" });
});
