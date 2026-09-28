// A UI/UX audit of the real console: behaviour a screenshot cannot show.
//
//   cd core && cargo build --bins
//   cd ../tests/ui && npm run audit          (or: node audit.mjs [out-dir])
//
// Starts a console of its own against a fake model that streams a short
// answer (and can refuse the key), drives it the way a person does -- the
// keyboard, a real turn, a bad key, a message too long, a phone -- and checks
// what they would notice. Each check prints PASS or FAIL; the run exits 1 on
// any failure, and writes a screenshot of each stage and a report to
// testbed/shots/audit.
//
// It sits beside showcase.mjs, which answers "does it look right" by eye.
// This answers "does it behave right", and every check here is something
// the first audit found broken. Colour contrast is not checked: the page's
// colours are color-mix() and custom properties that no naive reader gets
// right, and a checker that cries wolf is worse than none. Judge it in the
// showcase shots. Set PW_CHROMIUM to use a preinstalled Chromium.

import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";

import { chromium } from "@playwright/test";

import { repo, startConsole } from "./console.mjs";

const out = path.resolve(process.argv[2] ?? path.join(repo, "testbed", "shots", "audit"));
fs.mkdirSync(out, { recursive: true });

// ---------------------------------------------------------------- the model
let refuse = false;
const WORD_MS = 400;
const model = http.createServer((req, res) => {
  let body = "";
  req.on("data", c => (body += c));
  req.on("end", async () => {
    if (req.method === "GET") {
      res.writeHead(200, { "Content-Type": "application/json" });
      return res.end(JSON.stringify({ data: [{ id: "fake/model", context_length: 32000 }] }));
    }
    if (refuse) {
      res.writeHead(401, { "Content-Type": "application/json" });
      return res.end(JSON.stringify({ error: { message: "Invalid API key" } }));
    }
    res.writeHead(200, { "Content-Type": "text/event-stream" });
    for (const w of ["Opened ", "the ", "workbook."]) {
      res.write(`data: ${JSON.stringify({ choices: [{ delta: { content: w } }] })}\n\n`);
      await new Promise(r => setTimeout(r, WORD_MS));
    }
    res.end(`data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: "stop" }] })}\n\ndata: [DONE]\n\n`);
  });
});
await new Promise(r => model.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${model.address().port}`;
const home = fs.mkdtempSync(path.join(os.tmpdir(), "syn-audit-"));
const env = {
  AGENT_ENV_FILE: path.join(home, "no.env"), AGENT_HOME: home,
  AGENT_BASE_URL: base, AGENT_API_KEY: "sk-audit",
  AGENT_BASE_URL_STANDARD: base, AGENT_API_KEY_STANDARD: "sk-audit", AGENT_MODEL_STANDARD: "fake/model",
};

// ---------------------------------------------------------------- the report
const results = [];
function check(name, ok, detail = "") {
  results.push({ name, ok, detail });
  console.log(`  ${ok ? "PASS" : "FAIL"} ${name}${detail ? ": " + detail : ""}`);
}
const shot = (page, name) => page.screenshot({ path: path.join(out, name + ".png") });

const con = await startConsole({ port: 7851, env });
const browser = await chromium.launch({ executablePath: process.env.PW_CHROMIUM });
console.log(`== UI/UX audit of ${con.url}`);
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const errors = [];
  page.on("pageerror", e => errors.push(String(e)));
  page.on("console", m => m.type() === "error" && errors.push(m.text()));
  await page.goto(con.url);
  await page.waitForFunction(() => !!window.live);
  await page.waitForTimeout(800);
  await shot(page, "1-first-screen");

  // Every control a screen reader can name.
  const unnamed = await page.evaluate(() => {
    const name = e => (e.getAttribute("aria-label") || e.getAttribute("title") || e.textContent || e.getAttribute("placeholder") || "").trim();
    return [...document.querySelectorAll("button, a[href], input, select, textarea, [role=button]")]
      .filter(e => e.offsetParent !== null && !name(e))
      .map(e => e.outerHTML.slice(0, 80));
  });
  check("every visible control has an accessible name", !unnamed.length, unnamed.join(" | "));

  // Send says what it can do.
  const send = page.locator("#send");
  await page.locator("#box").fill("");
  check("send is off with nothing typed", await send.isDisabled());
  await page.locator("#box").fill("x");
  check("send is on once something is typed", await send.isEnabled());
  await page.locator("#box").fill("");

  // Settings in one click, from where a person looks.
  check("settings open from the sidebar", await page.locator("#setgear").isVisible());

  // The keyboard: every stop shows where focus is.
  await page.locator("#box").focus();
  const stops = [];
  for (let i = 0; i < 14; i++) {
    await page.keyboard.press("Tab");
    stops.push(await page.evaluate(() => {
      const e = document.activeElement;
      if (!e || e === document.body) return null;
      // A ring is anything about the element, or the two boxes around it,
      // that changes when focus leaves: the pill around a select, the
      // composer around the textarea.
      const chain = [e, e.parentElement, e.parentElement?.parentElement].filter(Boolean);
      const look = () => chain.map(n => { const s = getComputedStyle(n); return s.outlineStyle + s.outlineWidth + s.boxShadow + s.borderColor; }).join("|");
      const focused = look();
      e.blur();
      const blurred = look();
      e.focus({ focusVisible: true });
      const label = (e.getAttribute("aria-label") || e.textContent || e.id || e.tagName).trim().slice(0, 30);
      return { label, ring: focused !== blurred };
    }));
  }
  const ringless = [...new Set(stops.filter(s => s && !s.ring).map(s => s.label))];
  check("every tab stop shows focus", !ringless.length, ringless.join(", "));

  // A real turn: the pill, the button, how long, what is left afterwards.
  const pill = () => page.locator("#hands .lbl").innerText();
  await page.locator("#box").fill("open my budget workbook");
  const t0 = Date.now();
  await page.locator("#box").press("Enter");
  await page.waitForTimeout(WORD_MS);
  check("the pill says Syn is working during a run", (await pill()) === "Working", await pill());
  check("send turns into Stop during a run", (await send.getAttribute("aria-label")) === "Stop");
  await shot(page, "2-during-a-turn");
  await page.locator("#send:not(.stop)").waitFor({ timeout: 60_000 });
  const took = Date.now() - t0;
  // The model takes 3 x WORD_MS; the rest is Syn.
  check("a turn settles soon after the model finishes", took < 3 * WORD_MS + 2500, `${took} ms for a ${3 * WORD_MS} ms answer`);
  check("the pill is idle again afterwards", (await pill()) !== "Working", await pill());
  check("the reply lands in the thread", /workbook/.test(await page.locator("#tl .bot").last().innerText().catch(() => "")));
  check("the chat takes its title from the first message", (await page.locator("#title").innerText()).includes("budget"));
  await page.waitForTimeout(800);
  check("the chat is listed in the sidebar", (await page.locator("#chats").innerText()).includes("budget"));
  await shot(page, "3-after-a-turn");

  // A refused key: said where it is fixed.
  refuse = true;
  await page.locator("#box").fill("try again");
  await page.locator("#box").press("Enter");
  await page.locator("#send:not(.stop)").waitFor({ timeout: 60_000 });
  await page.waitForTimeout(500);
  const banners = await page.evaluate(() => window.live.banners);
  check("a refused key raises a notice that leads to Settings",
    banners.some(b => /key/i.test(b.title)) && (await page.locator("#banners button", { hasText: "Open settings" }).count()) > 0,
    JSON.stringify(banners));
  await shot(page, "4-refused-key");
  refuse = false;

  // A message too long for the console: refused plainly, and not lost.
  const long = "x".repeat(70_000);
  await page.locator("#box").fill(long);
  await page.locator("#box").press("Enter");
  await page.waitForTimeout(1500);
  const fail = await page.locator("#tl .fail").last().innerText().catch(() => "");
  check("an oversized message is refused in plain words", /64 KB/.test(fail) && !/unreachable/i.test(fail), fail.slice(0, 120));
  check("an oversized message stays in the box", (await page.locator("#box").inputValue()).length === long.length);
  await page.locator("#box").fill("");
  await shot(page, "5-oversized-message");

  // Nothing broke on the way, apart from the 413 asked for above.
  const real = errors.filter(e => !/413/.test(e));
  check("no errors in the page", !real.length, real.join(" | "));

  // A phone.
  const phone = await browser.newPage({ viewport: { width: 375, height: 740 }, isMobile: true, hasTouch: true });
  await phone.goto(con.url);
  await phone.waitForFunction(() => !!window.live);
  await phone.waitForTimeout(600);
  check("a phone never scrolls sideways", !(await phone.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1)));
  const small = await phone.evaluate(() => [...document.querySelectorAll("button, select")].filter(b => b.offsetParent).map(b => {
    const r = b.getBoundingClientRect();
    return { l: (b.getAttribute("aria-label") || b.textContent || b.id).trim().slice(0, 20), w: Math.round(r.width), h: Math.round(r.height) };
  }).filter(x => x.w < 32 || x.h < 32));
  check("phone tap targets are at least 32px", !small.length, small.map(s => `${s.l} ${s.w}x${s.h}`).join(", "));
  await shot(phone, "6-phone");
} finally {
  await browser.close();
  con.stop();
  model.close();
}

const failed = results.filter(r => !r.ok);
const md = [
  `# UI/UX audit`, "",
  `${results.length - failed.length} of ${results.length} checks passed.`, "",
  ...results.map(r => `- ${r.ok ? "PASS" : "**FAIL**"} ${r.name}${r.detail ? ` — ${r.detail}` : ""}`), "",
  "Screenshots of each stage are beside this file.",
].join("\n");
fs.writeFileSync(path.join(out, "report.md"), md + "\n");
console.log(`== ${failed.length ? failed.length + " failed" : "all passed"}; report and screenshots in ${out}`);
process.exit(failed.length ? 1 : 0);
