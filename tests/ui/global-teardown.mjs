import fs from "node:fs";

import { readState, stateFile, stopAllConsoles, urlFile } from "./console.mjs";

export default async function globalTeardown() {
  try {
    const { pid } = readState();
    // The console holds a CLI child of its own; killing the tree matters or
    // the next run finds the port still bound.
    process.kill(pid);
  } catch {
    // Already gone, or setup never got far enough to write the state file.
  }
  // Specs that took a console of their own stop it in a finally, but a
  // crash or a timeout skips that and the orphan then holds its port
  // against the next run.
  stopAllConsoles();
  fs.rmSync(stateFile, { force: true });
  fs.rmSync(urlFile, { force: true });
}
