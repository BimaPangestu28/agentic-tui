// @ts-check
const { test, expect } = require("@playwright/test");

// A single smoke test: the embedded web UI boots, the WASM app hydrates, and
// the shell renders. This proves the trunk build, the rust-embed asset
// serving, and the Leptos app all work end to end in a real browser. It does
// not drive a run (that needs the `claude` CLI); deeper flows are covered by
// the Rust integration tests.

test("the app shell loads and renders its nav", async ({ page }) => {
  await page.goto("/");

  await expect(page).toHaveTitle(/Agentic Orchestrator/);

  // The app-bar brand and the two primary nav links prove the WASM app
  // hydrated, not just that index.html was served.
  await expect(
    page.getByText("Agentic Orchestrator", { exact: false }),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: "Workspaces" })).toBeVisible();
  await expect(page.getByRole("link", { name: "New run" })).toBeVisible();
});
