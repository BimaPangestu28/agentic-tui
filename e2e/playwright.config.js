// @ts-check
const { defineConfig, devices } = require("@playwright/test");

// The server binds an ephemeral port by default; `AGENTIC_ADDR` pins it so the
// test has a stable URL to open. `--no-open` stops the binary from launching a
// real browser tab. The release binary must be built first (`make release`);
// the `test-e2e-browser` make target and CI job both do that.
const HOST = "127.0.0.1";
const PORT = 8137;
const BASE_URL = `http://${HOST}:${PORT}`;

module.exports = defineConfig({
  testDir: "./tests",
  timeout: 30_000,
  expect: { timeout: 10_000 },
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: [["list"]],
  use: {
    baseURL: BASE_URL,
    trace: "on-first-retry",
  },
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
  ],
  webServer: {
    command: "../target/release/agentic-tui --no-open",
    url: BASE_URL,
    env: { AGENTIC_ADDR: `${HOST}:${PORT}` },
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
});
