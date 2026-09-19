import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { here, repo, stateFile, urlFile } from "./console.mjs";

// A port of its own. The test must not steal the console the user is looking
// at on 7777, and must not be answered by one left over from a previous run.
const PORT = Number(process.env.SYN_UI_TEST_PORT ?? 7799);

async function waitForUrl(child) {
  for (let i = 0; i < 40; i++) {
    if (child.exitCode !== null) {
      throw new Error(`ui.exe exited with ${child.exitCode}: is port ${PORT} already in use?`);
    }
    if (fs.existsSync(urlFile)) {
      const first = fs.readFileSync(urlFile, "utf8").split("\n")[0].trim();
      if (first.startsWith("http://")) return first;
    }
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`the console never wrote its address to ${urlFile}`);
}

export default async function globalSetup() {
  const ui = path.join(repo, "core", "target", "debug", "ui.exe");
  if (!fs.existsSync(ui)) {
    throw new Error(`build it first: cd core && cargo build --bins  (missing ${ui})`);
  }
  fs.rmSync(urlFile, { force: true });
  fs.rmSync(stateFile, { force: true });

  // cwd is the repo root so the CLI child finds .env and .syn/chats where a
  // hand-launched console would.
  const child = spawn(ui, ["--port", String(PORT), "--url-file", urlFile], {
    cwd: repo,
    stdio: "ignore",
    windowsHide: true,
  });
  child.unref();

  const url = await waitForUrl(child);
  fs.writeFileSync(stateFile, JSON.stringify({ url, pid: child.pid, port: PORT }, null, 2));
  console.log(`console for tests: ${url}`);
  void here;
}
