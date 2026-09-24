// Is the live view actually live?
//
// The console used to poll `/events` every 700ms-2s. A read that took 40ms
// could sit invisible for most of a second, and a turn that asked for
// eight tools at once arrived as one clump rather than eight rows landing
// one after another. It did not feel like watching a run; it felt like
// refreshing a page.
//
// This measures it. A second console is started with `--tail` pointed at a
// file this spec owns, so appending a line here is exactly what a run in
// another process does, and the clock between the write and the row
// appearing is the real end-to-end latency. Nothing is mocked: the same
// `core::live::read_from` the server always used is doing the reading.

import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { test, expect } from "@playwright/test";

import { repo, uiBin } from "./console.mjs";

const PORT = Number(process.env.SYN_STREAM_TEST_PORT ?? 7801);

let child;
let url;
let logFile;

/** Append one CLI output line to the log the console is tailing. */
function emit(line) {
  fs.appendFileSync(logFile, line + "\n");
}

const step = (o) => "RECEIPT step " + JSON.stringify(o);

test.beforeAll(async () => {
  const ui = uiBin;
  if (!fs.existsSync(ui)) throw new Error(`build it first: cd core && cargo build --bins`);

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "syn-stream-"));
  logFile = path.join(dir, "run.log");
  // Three lines of banner: `live::newest` ignores a log that has never got
  // past one, and pinning with --tail should not depend on that rule, but
  // starting past it keeps the two paths comparable.
  fs.writeFileSync(logFile, "RECEIPT env ok\nRECEIPT session s\nRECEIPT registry 0\n");

  const urlFile = path.join(dir, "url.txt");
  child = spawn(ui, ["--port", String(PORT), "--url-file", urlFile, "--tail", logFile], {
    cwd: repo,
    stdio: "ignore",
    windowsHide: true,
  });
  child.unref();

  for (let i = 0; i < 40 && !url; i++) {
    if (fs.existsSync(urlFile)) {
      const first = fs.readFileSync(urlFile, "utf8").split("\n")[0].trim();
      if (first.startsWith("http://")) url = first;
    }
    if (!url) await new Promise((r) => setTimeout(r, 250));
  }
  if (!url) throw new Error(`the streaming console never came up on ${PORT}`);
});

test.afterAll(() => {
  try { child?.kill(); } catch { /* it is going away either way */ }
});

test.beforeEach(async ({ page }) => {
  await page.goto(url);
  await page.waitForFunction(() => !!window.live);
  // The stream has to be the transport under test, not the poller.
  await page.waitForFunction(() => window.live.sse === true, null, { timeout: 10_000 });
});

test("the page is fed by the stream, not by polling", async ({ page }) => {
  const state = await page.evaluate(() => ({
    sse: window.live.sse,
    open: window.live.streaming,
    poller: window.live.polling,
  }));
  expect(state.sse).toBe(true);
  expect(state.open).toBe(true);
  // Both transports appending to one timeline would double every row.
  expect(state.poller).toBeFalsy();
});

test("a line written by another process is on screen in well under a second", async ({ page }) => {
  const before = await page.locator(".act").count();

  const t0 = Date.now();
  emit(step({ label: "Read Sheet1!A1:B2", tool: "read", app: "excel", status: "done", detail: "2x2" }));
  await expect(page.locator(".act")).toHaveCount(before + 1, { timeout: 5000 });
  const latency = Date.now() - t0;

  // The server wakes every 60ms, so anything near a second means the
  // stream is not carrying this and the poller is.
  expect(latency).toBeLessThan(600);
  await expect(page.locator(".act .nm").last()).toHaveText("Read Sheet1!A1:B2");
});

test("eight calls in one turn arrive as eight rows, not one clump", async ({ page }) => {
  const before = await page.locator(".act").count();
  const seen = [];

  // Watch the DOM rather than the network: what matters is whether a
  // person sees the run progress, not how many frames arrived.
  await page.exposeFunction("rowSeen", (n) => seen.push({ n, at: Date.now() }));
  await page.evaluate(() => {
    const target = document.querySelector("#tl");
    new MutationObserver(() => window.rowSeen(document.querySelectorAll(".act").length)).observe(target, {
      childList: true,
      subtree: true,
    });
  });

  for (let i = 0; i < 8; i++) {
    emit(step({ label: `Read Sheet1!A${i}`, tool: "read", app: "excel", status: "done", detail: "ok" }));
    // Spaced like a real run's round trips through the pipe.
    await new Promise((r) => setTimeout(r, 120));
  }
  await expect(page.locator(".act")).toHaveCount(before + 8, { timeout: 5000 });

  // The rows must have appeared over time, not all at the end. With the
  // old 700ms poll, eight rows 120ms apart landed in one or two batches.
  const distinct = new Set(seen.map((s) => s.n)).size;
  expect(distinct).toBeGreaterThanOrEqual(6);
});

test("the stream survives a quiet minute", async ({ page }) => {
  // No traffic for longer than a browser's patience with an idle body.
  // The server's comment frame is what keeps it open; without it the page
  // silently stops being live and nothing says so.
  await page.waitForTimeout(20_000);
  const alive = await page.evaluate(() => window.live.streaming && window.live.sse);
  expect(alive).toBe(true);

  const before = await page.locator(".act").count();
  emit(step({ label: "Read Sheet1!Z9", tool: "read", app: "excel", status: "done", detail: "ok" }));
  await expect(page.locator(".act")).toHaveCount(before + 1, { timeout: 3000 });
});

test("a line already drawn is not drawn again when the stream connects", async ({ page }) => {
  // The bug a real run made obvious: the stream named its source on
  // connect, the page treated that as a change of run, and the server
  // replayed the whole log from zero on top of everything the first poll
  // had already drawn. Every step of the run appeared twice, which is
  // what a person actually saw on screen.
  // Labels unique to this test. An earlier test in this file already
  // emitted a row called "Read Sheet1!A1", and counting repeats by label
  // across the whole log calls that a duplicate when it is two real
  // lines that happen to read the same.
  emit(step({ label: "Read Dedupe!A1", tool: "read", app: "excel", status: "done", detail: "ok" }));
  emit(step({ label: "Added Dedupe", tool: "struct", app: "excel", status: "done", detail: "ok" }));

  // A page arriving after those lines were written: it polls, then the
  // stream connects on top.
  const fresh = await page.context().newPage();
  await fresh.goto(url);
  await fresh.waitForFunction(() => !!window.live);
  await fresh.waitForFunction(() => window.live.sse === true, null, { timeout: 10_000 });
  await fresh.waitForTimeout(600);

  // A page opening after those lines does NOT redraw them: the saved
  // transcript is the record of what happened, the live view is what is
  // happening. Drawing both put every step of a finished run on screen
  // twice, which is what the console actually looked like.
  const labels = await fresh.evaluate(() => window.live.rows.map((r) => r.text));
  expect(labels.filter((l) => l === "Read Dedupe!A1"), `rows=${JSON.stringify(labels)}`).toHaveLength(0);
  expect(labels.filter((l) => l === "Added Dedupe")).toHaveLength(0);

  // And a line written after it opened arrives, exactly once.
  emit(step({ label: "Wrote Dedupe!A1", tool: "write", app: "excel", status: "done", detail: "ok" }));
  await expect
    .poll(async () => (await fresh.evaluate(() => window.live.rows.map((r) => r.text))).filter((l) => l === "Wrote Dedupe!A1").length)
    .toBe(1);
  await fresh.close();
});

test("the poller carries the page when the stream cannot", async ({ page }) => {
  // A proxy that eats text/event-stream, or any client without
  // EventSource. The console must degrade, not go blank.
  await page.evaluate(() => window.live.toPolling(300));
  const before = await page.locator(".act").count();
  emit(step({ label: "Read Sheet1!Q1", tool: "read", app: "excel", status: "done", detail: "ok" }));
  await expect(page.locator(".act")).toHaveCount(before + 1, { timeout: 5000 });
});
