import { defineConfig, devices } from "@playwright/test";

/**
 * Browser check for published custom apps, run by
 * `.github/workflows/custom-app-checks.yaml` against staging and prod.
 *
 * No `webServer` and no `globalSetup`: the targets are apps already deployed,
 * named by `CUSTOM_APP_CHECK_TARGETS` (see tests/custom-app-checks/app-loads.spec.ts).
 */
export default defineConfig({
  testDir: "./tests/custom-app-checks",
  retries: process.env.CI ? 1 : 0,
  reporter: [["list"], ["html", { open: "never" }]],
  // The spec waits up to 30s for the ready beacon and then up to 30s for the
  // selector. The 30s default test timeout would cut that short and report a
  // timeout instead of the wait that actually failed.
  timeout: 90_000,
  use: {
    // Off, not retain-on-failure. A trace records every request's headers, the
    // spec sends X-API-Key, and the report is uploaded as a CI artifact.
    trace: "off",
    // Off for the same reason: a screenshot of a failed app is its data, and
    // nothing the checks upload may carry customer rows (record shape only).
    screenshot: "off",
    // The spec attaches X-API-Key by routing requests, and a request that a
    // service worker handles never reaches a route. Blocking the platform's
    // worker keeps every request behind that origin check. The worker is a
    // warm-path cache; a fresh browser takes the cold path either way.
    serviceWorkers: "block"
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }]
});
