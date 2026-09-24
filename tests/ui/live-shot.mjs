import { chromium } from "@playwright/test";
const b = await chromium.launch();
const p = await b.newPage({ viewport: { width: 1440, height: 900 }, colorScheme: "dark" });
await p.goto("http://127.0.0.1:7777/");
await p.waitForFunction(() => !!window.cli);
await p.evaluate(() => window.openChat("c1789891169-3110"));
await p.waitForFunction(() => !!window.live);
await p.waitForTimeout(2500);
const rows = await p.evaluate(() => window.live.rows.map(r => r.text));
const dupes = rows.filter((l, i) => rows.indexOf(l) !== i);
await p.screenshot({ path: "../../testbed/shots/live-run.png" });
console.log(JSON.stringify({ n: rows.length, dupes, turns: (await p.evaluate(() => window.live.turns)).length,
  map: await p.evaluate(() => window.live.mapReady), sse: await p.evaluate(() => window.live.sse),
  meter: await p.evaluate(() => document.querySelector("#meter .nums").textContent) }, null, 1));
await p.close(); await b.close();
