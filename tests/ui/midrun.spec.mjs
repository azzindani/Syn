import { expect, test } from "@playwright/test";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";

import { startConsole } from "./console.mjs";

// A page opened while a run is going. Found live: a person refreshed the
// console during a long analyst run and got a blank page -- no sidebar, no
// thread, nothing of the run -- because the page's first command queued
// behind the turn in the one CLI child and waited for all of it. A second
// fault hid behind the first: once the page did get going, it adopted the
// log's cursor and drew nothing of the turn so far, trusting a transcript
// that a running turn has not been written to yet.
//
// The fake model streams its answer slowly, so the turn is still running
// when the second page opens.
const WORDS = 30, WORD_MS = 400;
let model, stop, consoleUrl, home;

test.beforeAll(async () => {
  model = http.createServer((req, res) => {
    let body = "";
    req.on("data", c => (body += c));
    req.on("end", async () => {
      if (req.method === "GET") {
        res.writeHead(200, { "Content-Type": "application/json" });
        return res.end(JSON.stringify({ data: [{ id: "fake/model", context_length: 32000 }] }));
      }
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      for (let i = 0; i < WORDS; i++) {
        res.write(`data: ${JSON.stringify({ choices: [{ delta: { content: `word${i} ` } }] })}\n\n`);
        await new Promise(r => setTimeout(r, WORD_MS));
      }
      res.end(`data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: "stop" }] })}\n\ndata: [DONE]\n\n`);
    });
  });
  await new Promise(r => model.listen(0, "127.0.0.1", r));
  const base = `http://127.0.0.1:${model.address().port}`;
  home = fs.mkdtempSync(path.join(os.tmpdir(), "syn-midrun-"));
  ({ url: consoleUrl, stop } = await startConsole({
    port: Number(process.env.SYN_UI_MIDRUN_PORT ?? 7821),
    env: {
      AGENT_ENV_FILE: path.join(home, "no.env"), AGENT_HOME: home,
      AGENT_BASE_URL: base, AGENT_API_KEY: "sk-midrun",
      AGENT_BASE_URL_STANDARD: base, AGENT_API_KEY_STANDARD: "sk-midrun", AGENT_MODEL_STANDARD: "fake/model",
    },
  }));
});

test.afterAll(async () => {
  stop?.();
  await new Promise(r => model?.close(r));
});

test("a page opened during a run draws itself and the run, without waiting for it to end", async ({ context }) => {
  const first = await context.newPage();
  await first.goto(consoleUrl);
  await expect(first.locator("#mname")).toHaveText(/./);
  await first.locator("#box").fill("say something long");
  await first.locator("#box").press("Enter");
  await expect(first.locator("body.running, body.busy")).toHaveCount(1);

  // Well inside the turn: it lasts WORDS * WORD_MS, twelve seconds.
  const late = await context.newPage();
  const t = Date.now();
  await late.goto(consoleUrl);
  // Not the model pill: it says "Automatic" in the page's own markup, so it
  // reads fine on a page that never finished starting. The sidebar is only
  // filled once the page has had answers back from the console.
  await expect(late.locator("#chats > *")).not.toHaveCount(0, { timeout: 5_000 });
  await expect(late.locator("#tl .breath")).toBeVisible({ timeout: 5_000 });
  await expect(late.locator("#chats .chat")).not.toHaveCount(0, { timeout: 5_000 });
  expect(Date.now() - t, "the page waited for the turn").toBeLessThan(WORDS * WORD_MS - 2_000);

  // And it still sees the run through to its answer.
  await expect(late.locator("#tl .bot").last()).toContainText("word29", { timeout: 30_000 });
});
