// Settings > Appearance: text size and layout density, five levels each.
//
// The setting exists so Syn can be used as small as possible, so what is
// checked is the thing a person would notice: that nothing changes for
// someone who never opens it (level 4 of 5 IS the page as it was), that
// each step really is smaller or larger and in the right direction, that
// the choice is still there after a restart and is already applied when the
// page first paints, that it works from the keyboard, that no text goes
// under 9px at any level, and that at the smallest level the page still
// fits in the smallest window Syn's own frame allows (440x320).
//
// Its own console, with its own empty data folder, so no real key or chat
// is read: a first start with no key is also the state with the tallest
// composer, which is the worst case for the small window.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { expect, test } from "@playwright/test";

import { repo, startConsole } from "./console.mjs";

let url, stop, home;

test.beforeAll(async () => {
  home = fs.mkdtempSync(path.join(os.tmpdir(), "syn-appearance-"));
  const empty = path.join(home, "empty.env");
  fs.writeFileSync(empty, "");
  ({ url, stop } = await startConsole({
    port: Number(process.env.SYN_UI_APPEARANCE_PORT ?? 7823),
    env: {
      AGENT_HOME: home,
      AGENT_ENV_FILE: empty,
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

const step = (o) => "RECEIPT step " + JSON.stringify(o);

// A conversation with one of everything the page draws: a bubble, a card of
// rows (one refused), prose with a table, code, a quote and a list.
const TRANSCRIPT = [
  { role: "user", text: "Build a per-site scorecard from the solar data and put the top three on a slide." },
  {
    role: "calls",
    text: JSON.stringify([
      { id: "a", function: { name: "read", arguments: '{"handle":"excel:solar.xlsx:data","selector":"data!A1:H6"}' } },
      { id: "f", function: { name: "struct", arguments: '{"handle":"excel:solar.xlsx:Scorecard","verb":"addSheet","name":"Scorecard"}' } },
    ]),
  },
  { role: "tool", id: "a", text: "grid data: 6x8" },
  { role: "tool", id: "f", text: "already exists" },
  {
    role: "assistant",
    text:
      "The scorecard is on a new **Scorecard** sheet.\n\n## Top sites\n\n" +
      "| Site | Total kWh | Share |\n|---|---:|---:|\n| Bearspaw Water Treatment Plant | 3,082,638 | 31.3% |\n\n" +
      "> A quote worth keeping.\n\n```js\nconst x = 1;\n```\n\n- one\n- two **bold**\n\n[a link](https://example.com) and `code`",
  },
  { role: "user", text: "Now write the executive summary into the Word memo." },
];
const LABELS = {
  a: { text: "Read data!A1:H6 in solar.xlsx", status: "done", app: "excel" },
  f: { text: "Tried to add the sheet Scorecard", status: "refused", app: "excel" },
};
const LIVE = [
  step({ label: "Read memo.docx", tool: "read", app: "word", status: "done", detail: "4 paragraphs" }),
  step({ label: "Writing the summary paragraph", tool: "struct", app: "word", status: "running", detail: "" }),
];
const NOW = Math.floor(Date.now() / 1000);
const CHATS =
  "current=c1\n" +
  [
    { id: "c1", title: "Solar scorecard", updated: NOW - 30 },
    { id: "c2", title: "Hotel occupancy chart with a really long title that must be truncated", updated: NOW - 7200 },
  ]
    .map((o) => "CHAT " + JSON.stringify(o))
    .join("\n");

/** A page at `url` showing a finished turn and a running one, with two chats listed. */
async function staged(page, look) {
  if (look) {
    await page.addInitScript(
      ([t, d]) => localStorage.setItem("syn.look", JSON.stringify({ text: t, density: d })),
      [look.text, look.density],
    );
  }
  await page.route("**/cmd", (route) =>
    (route.request().postData() || "").trim() === "chat list" ? route.fulfill({ json: { out: CHATS } }) : route.continue(),
  );
  await page.goto(url);
  await page.waitForFunction(() => !!window.live);
  await page.evaluate(() => window.live.quiet());
  await page.evaluate(
    ([t, l, lines]) => {
      window.live.render(t, l);
      window.live.step("RECEIPT say model=x open=3");
      for (const x of lines) window.live.step(x);
    },
    [TRANSCRIPT, LABELS, LIVE],
  );
  await expect(page.locator(".act")).not.toHaveCount(0);
  await expect(page.locator(".chat")).toHaveCount(2);
  // The first-start notice arrives a moment after the page does; a page
  // measured before it is a different page from one measured after.
  await expect(page.locator('#banners .bn[data-banner-id="nokey"]')).toHaveCount(1);
}

const attrs = (page) =>
  page.evaluate(() => ({ text: document.documentElement.dataset.text, density: document.documentElement.dataset.density }));

/** A computed length in px, or the number the property holds. */
const metric = (page, sel, prop) =>
  page.locator(sel).first().evaluate((e, p) => parseFloat(getComputedStyle(e)[p]), prop);

/** Open Settings the way a person would: the key in the sidebar, or, where the sidebar is a closed drawer, the status menu. */
const openSettings = async (page) => {
  const gear = await page.locator("#setgear").boundingBox();
  const wide = page.viewportSize().width;
  if (gear && gear.x >= 0 && gear.x + gear.width <= wide) {
    await page.locator("#setgear").click();
  } else {
    await page.locator("#hands").click();
    await page.locator("#setbtn").click();
  }
  await expect(page.locator("#settings")).toHaveJSProperty("open", true);
};

/** Choose a level on one of the two scales, by clicking the real control. */
const choose = (page, axis, level) => page.locator(`#${axis}seg [data-level="${level}"]`).click();

const rising = (xs) => xs.every((x, i) => i === 0 || x > xs[i - 1]);
const notFalling = (xs) => xs.every((x, i) => i === 0 || x >= xs[i - 1]);

// What the page measured before the setting existed, at 1440x900. Level 4
// has to keep computing to exactly these: a person who never opens the
// setting must not see a change. (Checked against the old build pixel by
// pixel when the setting was added; these are the numbers to keep.)
const TODAY = [
  ["body", "fontSize", 14],
  ["#top", "height", 56],
  ["#brand", "height", 56],
  ["#side", "width", 264],
  ["#new", "height", 36],
  [".chat", "height", 34],
  [".chat", "paddingLeft", 10],
  ["#tl", "maxWidth", 736],
  ["#tl", "paddingTop", 30],
  ["#tl", "rowGap", 18],
  [".you", "paddingTop", 10],
  [".you", "paddingLeft", 15],
  [".turn", "paddingLeft", 40],
  [".turn", "rowGap", 12],
  [".bot", "fontSize", 14.5],
  [".acts-head", "minHeight", 40],
  [".act", "height", 28],
  [".act", "fontSize", 13],
  [".act .tile", "width", 20],
  ["#comp", "borderTopLeftRadius", 22],
  ["#comp", "maxWidth", 720],
  ["#box", "fontSize", 15],
  ["#box", "paddingLeft", 20],
  [".pick", "height", 30],
  [".pick", "fontSize", 12.5],
  ["#hands", "height", 32],
  ["#send", "width", 34],
  ["#send", "height", 34],
  ["#send svg", "width", 17],
  [".logo", "width", 28],
  [".logo svg", "width", 15],
];

test("with nothing stored the page is exactly the size it always was, level 4 of 5 on both scales", async ({ page }) => {
  await staged(page);
  expect(await attrs(page)).toEqual({ text: "4", density: "4" });
  expect(await page.evaluate(() => localStorage.getItem("syn.look"))).toBeNull();
  for (const [sel, prop, want] of TODAY) {
    expect(await metric(page, sel, prop), `${sel} ${prop}`).toBeCloseTo(want, 1);
  }
});

test("a stored value that is empty, broken or out of range means level 4, never an error", async ({ page }) => {
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(url);
  await page.waitForFunction(() => !!window.live);
  for (const bad of ["", "null", "{", '{"text":0,"density":9}', '{"text":"big"}', "[1,2]", '"3"', "7"]) {
    await page.evaluate((v) => localStorage.setItem("syn.look", v), bad);
    await page.reload();
    await page.waitForFunction(() => !!window.live);
    expect(await attrs(page), `stored ${JSON.stringify(bad)}`).toEqual({ text: "4", density: "4" });
  }
  // One good half is kept and the other falls back.
  await page.evaluate(() => localStorage.setItem("syn.look", '{"text":2}'));
  await page.reload();
  await page.waitForFunction(() => !!window.live);
  expect(await attrs(page)).toEqual({ text: "2", density: "4" });
  expect(errors).toEqual([]);
});

test("Appearance is in Settings, with a labelled five-step choice for each scale and a Reset", async ({ page }) => {
  await staged(page);
  await openSettings(page);
  const section = page.locator("#settings").getByRole("region", { name: "Appearance" });
  await expect(section).toBeVisible();
  for (const [name, noun] of [["Text size", "Text size"], ["Layout density", "Layout density"]]) {
    const group = section.getByRole("radiogroup", { name });
    await expect(group.getByRole("radio")).toHaveCount(5);
    await expect(group.getByRole("radio", { checked: true })).toHaveCount(1);
    await expect(group.getByRole("radio", { checked: true })).toHaveAccessibleName(`${noun}, level 4 of 5, Default`);
  }
  await expect(page.locator("#textval")).toHaveText("4 of 5 · Default");
  await expect(page.locator("#lookreset")).toBeDisabled();
});

test("each text level is strictly larger than the one before, and leaves the layout alone", async ({ page }) => {
  await staged(page);
  await openSettings(page);
  const sizes = { body: [], box: [], bot: [], act: [], btn: [] };
  const layout = [];
  for (const level of [1, 2, 3, 4, 5]) {
    await choose(page, "text", level);
    expect((await attrs(page)).text).toBe(String(level));
    sizes.body.push(await metric(page, "body", "fontSize"));
    sizes.box.push(await metric(page, "#box", "fontSize"));
    sizes.bot.push(await metric(page, ".bot", "fontSize"));
    sizes.act.push(await metric(page, ".act", "fontSize"));
    sizes.btn.push(await metric(page, "#lookreset", "fontSize"));
    layout.push([await metric(page, "#tl", "rowGap"), await metric(page, "#tl", "paddingTop"), await metric(page, "#comp", "borderTopLeftRadius")]);
  }
  for (const [name, xs] of Object.entries(sizes)) expect(rising(xs), `${name} ${xs}`).toBe(true);
  // Level 4 is today's, the smaller ones really are smaller and 5 is larger.
  expect(sizes.body[3]).toBeCloseTo(14, 1);
  expect(sizes.body[0]).toBeLessThan(sizes.body[3] * 0.8);
  expect(sizes.body[4]).toBeGreaterThan(sizes.body[3]);
  // Spacing belongs to the other scale.
  for (const l of layout) expect(l).toEqual(layout[0]);
});

test("each layout level is strictly roomier than the one before, and leaves the text alone", async ({ page }) => {
  await staged(page);
  await openSettings(page);
  const m = { gap: [], pad: [], radius: [], side: [], send: [], logo: [], top: [] };
  const rows = { act: [], chat: [], new: [], pick: [] };
  const text = [];
  for (const level of [1, 2, 3, 4, 5]) {
    await choose(page, "density", level);
    expect((await attrs(page)).density).toBe(String(level));
    m.gap.push(await metric(page, "#tl", "rowGap"));
    m.pad.push(await metric(page, "#tl", "paddingTop"));
    m.radius.push(await metric(page, "#comp", "borderTopLeftRadius"));
    m.side.push(await metric(page, "#side", "width"));
    m.send.push(await metric(page, "#send", "width"));
    m.logo.push(await metric(page, ".logo", "width"));
    m.top.push(await metric(page, "#top", "height"));
    rows.act.push(await metric(page, ".act", "height"));
    rows.chat.push(await metric(page, ".chat", "height"));
    rows.new.push(await metric(page, "#new", "height"));
    rows.pick.push(await metric(page, ".pick", "height"));
    text.push([await metric(page, "body", "fontSize"), await metric(page, ".act", "fontSize"), await metric(page, "#box", "fontSize")]);
  }
  for (const [name, xs] of Object.entries(m)) expect(rising(xs), `${name} ${xs}`).toBe(true);
  // A row never gets shorter than its own text needs, so at the tightest
  // two levels some rows stop shrinking together; they never grow back.
  for (const [name, xs] of Object.entries(rows)) {
    expect(notFalling(xs), `${name} ${xs}`).toBe(true);
    expect(xs[0], `${name} at 1 is smaller than at 4`).toBeLessThan(xs[3]);
    expect(xs[4], `${name} at 5 is larger than at 4`).toBeGreaterThan(xs[3]);
  }
  expect(m.side[3]).toBeCloseTo(264, 1);
  for (const t of text) expect(t).toEqual(text[0]);
});

test("the choice is kept across a reload and is already in force when the page first paints", async ({ page }) => {
  // Where the two attributes are at the moment <body> first exists: set by
  // the script in <head> they are there; set by the page's own script at
  // the end of the body they would not be, and the page would flash at
  // full size on every start.
  await page.addInitScript(() => {
    window.__atBody = null;
    new MutationObserver((_, mo) => {
      if (document.body && window.__atBody === null) {
        window.__atBody = { ...document.documentElement.dataset };
        mo.disconnect();
      }
    }).observe(document, { childList: true, subtree: true });
  });
  await staged(page);
  await openSettings(page);
  await choose(page, "text", 2);
  await choose(page, "density", 3);
  expect(JSON.parse(await page.evaluate(() => localStorage.getItem("syn.look")))).toEqual({ text: 2, density: 3 });
  const before = await metric(page, "body", "fontSize");

  await page.reload();
  await page.waitForFunction(() => !!window.live);
  expect(await page.evaluate(() => window.__atBody)).toMatchObject({ text: "2", density: "3" });
  expect(await attrs(page)).toEqual({ text: "2", density: "3" });
  expect(await metric(page, "body", "fontSize")).toBeCloseTo(before, 2);
  await openSettings(page);
  await expect(page.locator("#textseg").getByRole("radio", { checked: true })).toHaveText("2");
  await expect(page.locator("#densityseg").getByRole("radio", { checked: true })).toHaveText("3");
  await expect(page.locator("#textval")).toHaveText("2 of 5 · Smaller");
});

test("Reset goes back to level 4 on both and forgets the stored value", async ({ page }) => {
  await staged(page, { text: 1, density: 1 });
  await openSettings(page);
  await expect(page.locator("#lookreset")).toBeEnabled();
  await page.locator("#lookreset").click();
  expect(await attrs(page)).toEqual({ text: "4", density: "4" });
  expect(await page.evaluate(() => localStorage.getItem("syn.look"))).toBeNull();
  expect(await metric(page, "#top", "height")).toBeCloseTo(56, 1);
  await expect(page.locator("#lookreset")).toBeDisabled();
  // Focus is not left on the button that just switched itself off.
  expect(await page.evaluate(() => document.activeElement?.closest("#textseg") !== null)).toBe(true);
});

test("storage that throws leaves the page working at level 4, and a change still applies for the session", async ({ page }) => {
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.addInitScript(() => {
    const boom = () => {
      throw new DOMException("blocked", "SecurityError");
    };
    Storage.prototype.getItem = boom;
    Storage.prototype.setItem = boom;
    Storage.prototype.removeItem = boom;
  });
  await staged(page);
  expect(await attrs(page)).toEqual({ text: "4", density: "4" });
  await openSettings(page);
  await choose(page, "text", 2);
  await choose(page, "density", 1);
  expect(await attrs(page)).toEqual({ text: "2", density: "1" });
  expect(await metric(page, "#top", "height")).toBeLessThan(56);
  expect(errors).toEqual([]);
});

test("both scales work from the keyboard: arrows, Home, End, one tab stop each, a visible ring", async ({ page }) => {
  await staged(page);
  await openSettings(page);
  const textNow = () => page.evaluate(() => document.documentElement.dataset.text);
  const focusedLevel = () => page.evaluate(() => [document.activeElement.closest("[data-axis]")?.dataset.axis, document.activeElement.dataset.level]);

  // Only the level in force is a tab stop, so the group is one Tab, not five.
  await expect(page.locator('#textseg [role="radio"][tabindex="0"]')).toHaveCount(1);
  await expect(page.locator('#textseg [role="radio"][tabindex="-1"]')).toHaveCount(4);

  await page.locator('#textseg [aria-checked="true"]').focus();
  await page.keyboard.press("ArrowLeft");
  expect(await textNow()).toBe("3");
  expect(await focusedLevel()).toEqual(["text", "3"]);
  await page.keyboard.press("ArrowUp");
  expect(await textNow()).toBe("2");
  await page.keyboard.press("Home");
  expect(await textNow()).toBe("1");
  await page.keyboard.press("ArrowLeft");
  expect(await textNow(), "the scale stops at its ends").toBe("1");
  await page.keyboard.press("End");
  expect(await textNow()).toBe("5");
  await page.keyboard.press("ArrowRight");
  expect(await textNow()).toBe("5");
  await page.keyboard.press("ArrowDown");
  expect(await textNow()).toBe("5");
  await page.keyboard.press("ArrowLeft");
  expect(await textNow()).toBe("4");

  // The focus ring is drawn, and it is the page's own.
  const ring = await page.evaluate(() => {
    const s = getComputedStyle(document.activeElement);
    return { style: s.outlineStyle, width: parseFloat(s.outlineWidth) };
  });
  expect(ring.style).not.toBe("none");
  expect(ring.width).toBeGreaterThanOrEqual(2);

  // Tab moves on to the other scale, onto its level in force.
  await page.keyboard.press("Tab");
  expect(await focusedLevel()).toEqual(["density", "4"]);
  await page.keyboard.press("ArrowRight");
  expect((await attrs(page)).density).toBe("5");
  await page.keyboard.press("Home");
  expect((await attrs(page)).density).toBe("1");

  // And on to Reset, which Space or Enter presses.
  await page.keyboard.press("Tab");
  await expect(page.locator("#lookreset")).toBeFocused();
  await page.keyboard.press("Enter");
  expect(await attrs(page)).toEqual({ text: "4", density: "4" });
});

/** Every element that shows text, with the size it shows it at. Placeholders and pseudo-element text count. */
function textSizes() {
  const out = [];
  const all = document.querySelectorAll("body *");
  all.forEach((e, i) => {
    if (["SCRIPT", "STYLE", "OPTION", "svg", "path"].includes(e.tagName) || e.closest("svg")) return;
    const cs = getComputedStyle(e);
    if (cs.visibility === "hidden" || cs.display === "none" || e.getClientRects().length === 0) return;
    const own = [...e.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim() !== "");
    const field = (e.tagName === "TEXTAREA" || e.tagName === "INPUT") && (e.placeholder || e.value);
    if (own || field) out.push({ i, name: e.tagName.toLowerCase() + (e.id ? "#" + e.id : "") + (e.className && typeof e.className === "string" ? "." + e.className.split(" ")[0] : ""), size: parseFloat(cs.fontSize) });
    for (const p of ["::before", "::after"]) {
      const c = getComputedStyle(e, p);
      if (c.content && !["none", "normal", '""', "''"].includes(c.content) && c.display !== "none" && !c.content.startsWith("url(")) {
        out.push({ i, name: e.tagName.toLowerCase() + p, size: parseFloat(c.fontSize) });
      }
    }
  });
  return out;
}

/** The staged page with a pop-up open, so its text is on screen too. */
async function withPanels(page, look, panel) {
  await staged(page, look);
  await page.evaluate(() => window.live.ask({ preview: "hostname", why: "to label the report" }));
  if (panel === "settings") {
    await openSettings(page);
    await expect(page.locator(".krow")).toHaveCount(3);
  }
  if (panel === "pop") await page.locator("#hands").click();
  if (panel === "models") await page.locator("#mpick").click();
  if (panel === "spaces") await page.locator("#wpick").click();
  await page.waitForTimeout(250);
}

// The status menu and Settings are the same elements every time, so each
// can be paired element by element across levels. The two pickers are
// filled from the providers and from the disk, which a spec must not
// depend on: for them only the floor is checked.
for (const panel of ["settings", "pop", "models", "spaces"]) {
  const paired = panel === "settings" || panel === "pop";
  test(`no text is under 9px at any level${paired ? ", and every size shrinks at the smallest one" : ""} (${panel} open)`, async ({ page }) => {
    const at = {};
    for (const [name, look] of [["1/1", { text: 1, density: 1 }], ["4/4", { text: 4, density: 4 }], ["5/5", { text: 5, density: 5 }], ["1/5", { text: 1, density: 5 }]]) {
      await withPanels(page, look, panel);
      at[name] = await page.evaluate(textSizes);
    }
    // Enough was measured for the rest to mean something.
    expect(at["4/4"].length).toBeGreaterThan(paired ? 25 : 15);
    for (const [name, list] of Object.entries(at)) {
      for (const t of list) expect(t.size, `${t.name} at ${name}`).toBeGreaterThanOrEqual(8.99);
    }
    const top = (list) => Math.max(...list.map((t) => t.size));
    expect(top(at["1/1"])).toBeLessThan(top(at["4/4"]));
    expect(top(at["5/5"])).toBeGreaterThan(top(at["4/4"]));
    if (!paired) return;
    // The same elements, paired: at level 1 each is smaller than it is
    // today, unless today's is already at the floor. This is what catches
    // a size somebody wrote in plain pixels.
    expect(at["1/1"].length).toBe(at["4/4"].length);
    at["4/4"].forEach((t, k) => {
      const small = at["1/1"][k];
      expect(small.i).toBe(t.i);
      if (t.size > 9.01) expect(small.size, `${t.name}: ${t.size}px today`).toBeLessThan(t.size);
      expect(at["5/5"][k].size, `${t.name} at 5/5`).toBeGreaterThanOrEqual(t.size);
    });
    // Layout density does not touch text.
    at["1/5"].forEach((t, k) => expect(t.size).toBeCloseTo(at["1/1"][k].size, 2));
  });
}

test("no rule in the stylesheet writes a text size in plain pixels", async () => {
  // The type scale in :root is the one place that does; a new rule that
  // wrote `font-size:13px` would not move with the setting, and nothing
  // drawn in a state this spec does not stage would show it.
  const html = fs.readFileSync(path.join(repo, "widget", "index.html"), "utf8").replace(/\r/g, "");
  const css = html.slice(html.indexOf("<style>"), html.indexOf("</style>")).replace(/\/\*[\s\S]*?\*\//g, "");
  const bad = [];
  for (const m of css.matchAll(/(?:^|[;{\s])(font-size|font|line-height)\s*:\s*([^;}]*)/g)) {
    const value = m[2].replace(/max\(9px,/g, "");
    if (/\d(\.\d+)?px/.test(value)) bad.push(`${m[1]}:${m[2].trim()}`);
  }
  expect(bad).toEqual([]);
});

for (const scheme of ["dark", "light"]) {
  test(`at level 1 and 1 in the smallest window, 440x320, everything fits and nothing is cut off (${scheme})`, async ({ page }) => {
    await page.setViewportSize({ width: 440, height: 320 });
    await page.emulateMedia({ colorScheme: scheme });
    await staged(page, { text: 1, density: 1 });
    await page.waitForTimeout(200);

    const inside = async (sel, what) => {
      const r = await page.locator(sel).first().boundingBox();
      expect(r, `${what} is on the page`).not.toBeNull();
      expect(r.x, `${what} left`).toBeGreaterThanOrEqual(-0.5);
      expect(r.y, `${what} top`).toBeGreaterThanOrEqual(-0.5);
      expect(r.x + r.width, `${what} right`).toBeLessThanOrEqual(440.5);
      expect(r.y + r.height, `${what} bottom`).toBeLessThanOrEqual(320.5);
      return r;
    };

    // The page itself neither scrolls sideways nor hides anything that sticks out.
    const overflow = await page.evaluate(() => ({
      doc: document.documentElement.scrollWidth - innerWidth,
      body: document.body.scrollWidth - innerWidth,
      stuck: [...document.querySelectorAll("#top, #dock, #comp, #bar, #scroll")]
        .filter((e) => e.scrollWidth > e.clientWidth + 1)
        .map((e) => e.id),
    }));
    expect(overflow.doc).toBeLessThanOrEqual(0);
    expect(overflow.body).toBeLessThanOrEqual(0);
    expect(overflow.stuck).toEqual([]);

    // The composer: the box, every chip and the send button.
    await inside("#box", "the message box");
    const send = await inside("#send", "the send button");
    expect(send.width, "the send button is still big enough to hit").toBeGreaterThanOrEqual(14);
    for (const sel of ["#mpick", "#thinkpick", "#wpick"]) await inside(sel, sel);
    await inside("#hands", "the status pill");
    await inside("#toggle", "the sidebar toggle");
    const comp = await page.locator("#comp").boundingBox();
    for (const sel of ["#mpick", "#thinkpick", "#wpick", "#send"]) {
      const r = await page.locator(sel).boundingBox();
      expect(r.x + r.width, `${sel} stays inside the composer`).toBeLessThanOrEqual(comp.x + comp.width + 0.5);
    }
    // No two of the composer's controls sit on top of each other.
    const boxes = [];
    for (const sel of ["#mpick", "#thinkpick", "#wpick", "#send"]) boxes.push([sel, await page.locator(sel).boundingBox()]);
    for (let a = 0; a < boxes.length; a++) {
      for (let b = a + 1; b < boxes.length; b++) {
        const [na, A] = boxes[a], [nb, B] = boxes[b];
        const apart = A.x + A.width <= B.x + 0.5 || B.x + B.width <= A.x + 0.5 || A.y + A.height <= B.y + 0.5 || B.y + B.height <= A.y + 0.5;
        expect(apart, `${na} and ${nb} overlap`).toBe(true);
      }
    }

    // The sidebar is a drawer here, and opens inside the window.
    await page.locator("#toggle").click();
    await page.waitForTimeout(300);
    const side = await inside("#side", "the sidebar drawer");
    expect(side.width).toBeLessThan(440);
    await page.locator("#scrim").click({ position: { x: 430, y: 100 } });

    // The status menu, Settings and both pickers open inside the window, top and all.
    await page.locator("#hands").click();
    await page.waitForTimeout(250);
    await inside("#pop", "the status menu");
    await page.mouse.click(220, 3);
    await page.locator("#mpick").click();
    await page.waitForTimeout(300);
    await inside("#msearch", "the model search field");
    await page.keyboard.press("Escape");
    await page.locator("#wpick").click();
    await page.waitForTimeout(300);
    await inside("#wpath", "the folder field");
    await page.keyboard.press("Escape");
    await openSettings(page);
    await page.waitForTimeout(250);
    await inside("#settings", "the settings dialog");
    await page.locator("#look").scrollIntoViewIfNeeded();
    for (const axis of ["text", "density"]) {
      await page.locator(`#${axis}seg`).scrollIntoViewIfNeeded();
      const r = await page.locator(`#${axis}seg`).boundingBox();
      expect(r.x).toBeGreaterThanOrEqual(0);
      expect(r.x + r.width, `the ${axis} control fits the dialog`).toBeLessThanOrEqual(440);
      expect(r.height, `the ${axis} control is still big enough to hit`).toBeGreaterThanOrEqual(18);
    }
  });
}

for (const scheme of ["dark", "light"]) {
  test(`the choice reads in both themes: the level in force stands out from the others (${scheme})`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: scheme });
    await staged(page);
    await openSettings(page);
    for (const level of [1, 3, 5]) {
      await choose(page, "text", level);
      await choose(page, "density", level);
      expect(await page.evaluate(() => document.documentElement.dataset.theme)).toBe(scheme);
      for (const axis of ["text", "density"]) {
        const r = await page.locator(`#${axis}seg`).boundingBox();
        expect(r.width).toBeGreaterThan(100);
        expect(r.height).toBeGreaterThan(14);
        const [on, off] = await page.evaluate((ax) => {
          const bg = (e) => getComputedStyle(e).backgroundColor;
          const g = document.getElementById(ax + "seg");
          return [bg(g.querySelector('[aria-checked="true"]')), bg(g.querySelector('[aria-checked="false"]'))];
        }, axis);
        expect(on, `${axis} ${level} in ${scheme}`).not.toBe(off);
      }
    }
  });
}
