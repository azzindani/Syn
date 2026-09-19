import fs from "node:fs";

import { readState, stateFile, urlFile } from "./console.mjs";

export default async function globalTeardown() {
  try {
    const { pid } = readState();
    // The console holds a CLI child of its own; killing the tree matters or
    // the next run finds the port still bound.
    process.kill(pid);
  } catch {
    // Already gone, or setup never got far enough to write the state file.
  }
  fs.rmSync(stateFile, { force: true });
  fs.rmSync(urlFile, { force: true });
}
