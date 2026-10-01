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
let model, stop, consoleUrl, home, consoleEnv;
// Requests already turned away, by size: the retry is the same request.
const limited = new Set();

test.beforeAll(async () => {
  model = http.createServer((req, res) => {
    let body = "";
    req.on("data", c => (body += c));
    req.on("end", async () => {
      if (req.method === "GET") {
        res.writeHead(200, { "Content-Type": "application/json" });
        return res.end(JSON.stringify({ data: [{ id: "fake/model", context_length: 32000 }] }));
      }
      // A message asking for it is turned away once, as a provider at its
      // rate limit does, and answered when the console asks again.
      if (body.includes("rate me") && !limited.has(body.length)) {
        limited.add(body.length);
        res.writeHead(429, { "Content-Type": "application/json" });
        return res.end(JSON.stringify({ error: { message: "Rate limit exceeded. Please retry after a brief wait." } }));
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
  consoleEnv = {
    AGENT_ENV_FILE: path.join(home, "no.env"), AGENT_HOME: home,
    AGENT_BASE_URL: base, AGENT_API_KEY: "sk-midrun",
    AGENT_BASE_URL_STANDARD: base, AGENT_API_KEY_STANDARD: "sk-midrun", AGENT_MODEL_STANDARD: "fake/model",
  };
  ({ url: consoleUrl, stop } = await startConsole({
    port: Number(process.env.SYN_UI_MIDRUN_PORT ?? 7821),
    env: consoleEnv,
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
  // The reply arriving, word by word. Not the breathing dots: CSS hides
  // them the moment the reply starts to stream, so whether they are up
  // depends on when anyone looks.
  await expect(late.locator("#tl .bot").last()).toContainText("word", { timeout: 5_000 });
  await expect(late.locator("#chats .chat")).not.toHaveCount(0, { timeout: 5_000 });
  expect(Date.now() - t, "the page waited for the turn").toBeLessThan(WORDS * WORD_MS - 2_000);
  // The question as well as the work. It was sent from the other page, so
  // this one has it only from the run's own announcement.
  await expect(late.locator("#tl .you")).toHaveText(["say something long"], { timeout: 5_000 });

  // And it still sees the run through to its answer.
  await expect(late.locator("#tl .bot").last()).toContainText("word29", { timeout: 30_000 });
  // The page that sent it drew its own bubble, and does not draw a second.
  await expect(first.locator("#tl .you")).toHaveText(["say something long"]);
});

test("a page left open while the console restarts shows the next run without a refresh", async ({ page, request }) => {
  await page.goto(consoleUrl);
  await expect(page.locator("#chats > *")).not.toHaveCount(0);
  // The console goes away and comes back on the same address, as it does
  // when someone restarts it. Nobody touches the page.
  stop();
  await page.waitForTimeout(3_000);
  ({ url: consoleUrl, stop } = await startConsole({ port: Number(process.env.SYN_UI_MIDRUN_PORT ?? 7821), env: consoleEnv }));
  await page.waitForTimeout(3_000);
  const sent = request.post(consoleUrl + "cmd", {
    headers: { Origin: consoleUrl.replace(/\/$/, ""), "Content-Type": "text/plain" },
    data: "say after the restart",
    timeout: 60_000,
  });
  // A restarted console is on a chat of its own, so the page shows that one:
  // the new run alone, not appended under the conversation from before.
  await expect(page.locator("#tl .you")).toHaveText(["after the restart"], { timeout: 20_000 });
  await expect(page.locator("#tl .bot").last()).toContainText("word29", { timeout: 30_000 });
  await expect(page.locator("#tl .bot").last()).toContainText("word29", { timeout: 30_000 });
  await sent;
});

test("a run killed with its console leaves the page ready, on the console's chat, without a refresh", async ({ page, request }) => {
  const port = Number(process.env.SYN_UI_MIDRUN_PORT ?? 7821);
  const origin = consoleUrl.replace(/\/$/, "");
  const post = (data) => request.post(consoleUrl + "cmd", { headers: { Origin: origin, "Content-Type": "text/plain" }, data, timeout: 60_000 });
  await page.goto(consoleUrl);
  await expect(page.locator("#chats > *")).not.toHaveCount(0);
  // A run under way, then the console killed under it -- as Stop-Process
  // does -- and a new one started and pointed at a new chat from outside.
  void post("say a run that is killed").catch(() => {});
  await expect(page.locator("body.running")).toHaveCount(1, { timeout: 10_000 });
  stop();
  await page.waitForTimeout(2_000);
  ({ url: consoleUrl, stop } = await startConsole({ port, env: consoleEnv }));
  await post("chat new");

  // Nobody touches the page. It must stop claiming a run, and show the chat
  // the console is on now.
  await expect(page.locator("body.running, body.busy")).toHaveCount(0, { timeout: 20_000 });
  await expect(page.locator("#title")).toHaveText("New chat", { timeout: 20_000 });

  // And the next run, from anywhere, appears in it as it happens.
  const sent = post("say the next one");
  await expect(page.locator("#tl .you")).toHaveText(["the next one"], { timeout: 20_000 });
  await expect(page.locator("#tl .bot").last()).toContainText("word29", { timeout: 30_000 });
  await sent;
});

test("a message sent from outside the page shows in the thread as it runs", async ({ page, request }) => {
  await page.goto(consoleUrl);
  await expect(page.locator("#chats > *")).not.toHaveCount(0);
  const replies = await page.locator("#tl .bot").count();
  // Typed into the console by something that is not this page: a script, a
  // terminal, another tool. The page is only watching.
  const sent = request.post(consoleUrl + "cmd", {
    headers: { Origin: consoleUrl.replace(/\/$/, ""), "Content-Type": "text/plain" },
    data: "say a message from elsewhere",
    timeout: 60_000,
  });
  await expect(page.locator("#tl .you").last()).toHaveText("a message from elsewhere", { timeout: 10_000 });
  await expect(page.locator("#tl .bot")).toHaveCount(replies + 1, { timeout: 10_000 });
  await sent;
});

test("messages from outside each get their own turn, in order, through a rate limit", async ({ page, request }) => {
  // Found live, driving a session from a script: the second message's bubble
  // went in at the top, above the first turn, and its work and answer went
  // inside the first turn's card. A run that is sent from elsewhere starts
  // before its bubble arrives, and it had joined the turn before it. The
  // same session hit a rate limit, drawn as a red failure in the middle of a
  // turn that then answered.
  test.setTimeout(120_000);
  const post = (data) => request.post(consoleUrl + "cmd", {
    headers: { Origin: consoleUrl.replace(/\/$/, ""), "Content-Type": "text/plain" },
    data,
    timeout: 90_000,
  });
  await post("chat new");
  await page.goto(consoleUrl);
  await expect(page.locator("#chats > *")).not.toHaveCount(0);
  await expect(page.locator("#title")).toHaveText("New chat");

  await post("say the first message");
  const second = post("say the second message, rate me");
  await expect(page.locator(".bn")).toContainText("The model is busy", { timeout: 15_000 });
  await second;
  await expect(page.locator("#tl .bot").last()).toContainText("word29", { timeout: 30_000 });

  // you, Syn, you, Syn: each answer under its own question.
  const shape = await page.locator("#tl > *").evaluateAll(ns => ns.map(n =>
    n.classList.contains("you") ? "you:" + n.textContent.trim() :
    n.classList.contains("turn") ? "turn:" + n.querySelectorAll(".bot").length : n.className));
  expect(shape).toEqual(["you:the first message", "turn:1", "you:the second message, rate me", "turn:1"]);
  // Waited out, not failed.
  await expect(page.locator("#tl .fail")).toHaveCount(0);
  await expect(page.locator(".bn", { hasText: "The model is busy" })).toHaveCount(0);
});
