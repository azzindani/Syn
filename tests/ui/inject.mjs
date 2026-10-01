// Send one prompt to a console that is already running and print what comes
// back. This drives the real page rather than the CLI, so it exercises the
// composer, the turn indicator, the failover note and the markdown pass.
//
//   node inject.mjs "your prompt"
//   node inject.mjs "your prompt" --headed --shot ../../testbed/inject.png
//   node inject.mjs "..." --setup "hand synuia uia" --setup "live uia:poem::doc"
//   node inject.mjs "..." --model "Provider tab|model-id" --workspace D:\data
//
// --setup runs a console command through the page before the prompt, which
// is the only way to reach the CLI child the console owns: a second cli.exe
// has its own Runner and its own registry.
//
// --model and --workspace go through the pickers beside the box, by click,
// as a person would. A `--setup "model ..."` does not stick: send() types
// `model auto` before every message unless the picker holds a choice, so a
// whole run once went to the .env slot while the setup line said otherwise.
//
// The URL comes from the file console.ps1 writes, so the token is never
// guessed and never pasted.

import fs from "node:fs";
import path from "node:path";

import { chromium } from "@playwright/test";

import { repo } from "./console.mjs";

const argv = process.argv.slice(2);
const flag = (name) => {
  const i = argv.indexOf(name);
  return i === -1 ? null : argv[i + 1] ?? true;
};
const flags = (name) => argv.flatMap((a, i) => (a === name && argv[i + 1] ? [argv[i + 1]] : []));
const prompt = argv.filter((a, i) => !a.startsWith("--") && !String(argv[i - 1] ?? "").startsWith("--")).join(" ");
if (!prompt) {
  console.error('usage: node inject.mjs "your prompt" [--url <url>] [--headed] [--shot <png>]');
  process.exit(2);
}

const url = flag("--url") ?? (() => {
  const f = path.join(repo, "testbed", "console-url.txt");
  if (!fs.existsSync(f)) {
    throw new Error(`no console is running: ${f} is missing (start one with scripts/console.ps1)`);
  }
  return fs.readFileSync(f, "utf8").split("\n")[0].trim();
})();

const browser = await chromium.launch({ headless: !flag("--headed") });
const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, colorScheme: "dark" });
try {
  await page.goto(url, { timeout: 20_000 });
  await page.locator("#brand").waitFor({ timeout: 10_000 });
  await page.locator("#new").click();

  for (const cmd of flags("--setup")) {
    const out = await page.evaluate((c) => cli(c), cmd);
    const lines = out.trim().split(/\r?\n/).filter(Boolean);
    console.log(`setup: ${cmd}`);
    for (const l of lines) console.log(`  ${l}`);
    if (lines.some((l) => l.startsWith("ERROR"))) {
      throw new Error(`setup failed: ${cmd}`);
    }
  }

  const model = flag("--model");
  if (model) {
    const [tab, id] = String(model).split("|");
    await page.locator("#mpick").click();
    await page.locator("#mtabs .mchip", { hasText: tab }).first().click();
    await page.locator("#msearch").fill(id);
    // Exact id, not a search hit: an id such as "model-1.3" is also a prefix of
    // "model-1.3-preview".
    await page.locator("#mlist .mrow").filter({ has: page.locator(".mi", { hasText: new RegExp(`^${id.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`) }) }).first().click();
    console.log(`model: ${(await page.locator("#mname").innerText()).trim()} (${tab})`);
  }
  const ws = flag("--workspace");
  if (ws) {
    await page.locator("#wpick").click();
    await page.locator("#wpath").fill(String(ws));
    await page.locator("#wgo").click();
    await page.waitForFunction((w) => document.querySelector("#wname")?.textContent?.trim() !== "Everywhere", ws, { timeout: 10_000 });
    console.log(`workspace: ${(await page.locator("#wname").innerText()).trim()}`);
  }

  const box = page.locator("#box");
  await box.fill(prompt);
  await box.press("Enter");

  // --keep: a person is watching this window. Send, then leave the page to
  // the run and do nothing else to it; this process ends when the window is
  // closed by hand.
  if (flag("--keep")) {
    console.log("sent; the window stays open until you close it");
    await new Promise((r) => browser.on("disconnected", r));
    process.exit(0);
  }

  // Settled means the page no longer counts a run as live: `busy` is the
  // turn it sent, `running` any run the feed reports. The send button is
  // not the signal any more -- during a run it becomes Stop, enabled, and
  // waiting on it returned the moment the prompt went out.
  // A real analyst task runs for minutes, not seconds.
  const waitMs = Number(flag("--wait") ?? 900_000);
  const live = () => document.body.classList.contains("busy") || document.body.classList.contains("running");
  const idle = () => !document.body.classList.contains("busy") && !document.body.classList.contains("running");
  await page.waitForFunction(live, null, { timeout: 30_000 }).catch(() => {});
  await page.waitForFunction(idle, null, { timeout: waitMs, polling: 2_000 });

  const turn = await page.evaluate(() => ({
    acts: [...document.querySelectorAll("#tl .act")].map((a) => ({
      name: a.querySelector(".nm")?.textContent ?? "",
      detail: a.querySelector(".dt")?.textContent ?? "",
      failed: Boolean(a.querySelector(".bad")),
    })),
    notes: [...document.querySelectorAll("#tl .note")].map((n) => n.textContent),
    fails: [...document.querySelectorAll("#tl .fail")].map((n) => n.textContent),
    // Not `.bot:last-of-type`: that asks for the last div among siblings,
    // which is whatever the turn ended with, not the last reply.
    reply: [...document.querySelectorAll("#tl .bot")].pop()?.innerText ?? "",
  }));

  console.log(`\nprompt\n  ${prompt}\n`);
  for (const a of turn.acts) {
    console.log(`  ${a.failed ? "✗" : "·"} ${a.name} ${a.detail}`.trimEnd());
  }
  if (turn.acts.length) console.log("");
  for (const n of turn.notes) console.log(`note: ${n}`);
  for (const f of turn.fails) console.log(`fail: ${f}`);
  console.log(`reply\n${turn.reply.split("\n").map((l) => "  " + l).join("\n")}\n`);

  const shot = flag("--shot");
  if (shot) {
    await page.screenshot({ path: shot });
    console.log(`shot: ${shot}`);
  }
} finally {
  await browser.close();
}
