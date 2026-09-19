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
