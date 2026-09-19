import { defineConfig } from "@playwright/test";

// One worker: every spec talks to the same console process, which holds one
// CLI child with one current chat. Parallel specs would fight over it.
export default defineConfig({
  testDir: ".",
  globalSetup: "./global-setup.mjs",
  globalTeardown: "./global-teardown.mjs",
  workers: 1,
  timeout: 180_000,
  expect: { timeout: 15_000 },
  reporter: [["list"]],
  outputDir: "../../testbed/playwright",
  use: {
    viewport: { width: 1440, height: 900 },
    colorScheme: "dark",
    screenshot: "only-on-failure",
  },
});
