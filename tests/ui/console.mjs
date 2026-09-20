import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const here = path.dirname(fileURLToPath(import.meta.url));
export const repo = path.resolve(here, "..", "..");
export const urlFile = path.join(here, ".console-url.txt");
export const stateFile = path.join(here, ".console.json");

export function readState() {
  return JSON.parse(fs.readFileSync(stateFile, "utf8"));
}

/**
 * Start a console of this test file's own, and return its URL and a stop.
 *
 * Some checks are destructive to the process they run in: `kill` latches
 * the Runner for good ("fresh guard required"), which is exactly the
 * behaviour worth testing and exactly what makes it unrunnable against the
 * console every other spec is sharing. One `kill` used to take the rest of
 * the file down with it, and the failures it caused looked like bugs in
 * unrelated tests.
 */
export async function startConsole({ port, tail } = {}) {
  const { spawn } = await import("node:child_process");
  const os = await import("node:os");
  const ui = path.join(repo, "core", "target", "debug", "ui.exe");
  if (!fs.existsSync(ui)) throw new Error("build it first: cd core && cargo build --bins");

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "syn-console-"));
  const urlFile = path.join(dir, "url.txt");
  const args = ["--port", String(port), "--url-file", urlFile];
  if (tail) args.push("--tail", tail);

  const child = spawn(ui, args, { cwd: repo, stdio: "ignore", windowsHide: true });
  child.unref();

  let url = null;
  for (let i = 0; i < 40 && !url; i++) {
    // A console that exits at once has almost always found its port taken,
    // usually by an orphan from an interrupted run whose `stop()` never
    // got to run. Say that. "It never came up" sends you looking at the
    // wrong thing for ten minutes, which it did.
    if (child.exitCode !== null) {
      throw new Error(
        `the console on port ${port} exited with ${child.exitCode} before it was ready. ` +
          "Something is probably already listening there — an orphaned ui.exe from an " +
          "interrupted run. On Windows: taskkill /F /IM ui.exe",
      );
    }
    if (fs.existsSync(urlFile)) {
      const first = fs.readFileSync(urlFile, "utf8").split("\n")[0].trim();
      if (first.startsWith("http://")) url = first;
    }
    if (!url) await new Promise((r) => setTimeout(r, 250));
  }
  if (!url) throw new Error(`the console on port ${port} never answered and never exited`);
  started.add(child);
  return {
    url,
    dir,
    stop: () => {
      started.delete(child);
      try { child.kill(); } catch { /* going away anyway */ }
    },
  };
}

/** Consoles this run started, so teardown can sweep any a crash left behind. */
const started = new Set();

export function stopAllConsoles() {
  for (const c of started) {
    try { c.kill(); } catch { /* going away anyway */ }
  }
  started.clear();
}
