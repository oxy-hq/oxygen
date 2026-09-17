import { expect, test } from "@playwright/test";

// Loads each published custom app in a real browser and asserts it mounted.
// A custom-app host answers 200 with the app shell for every path, so a status
// code proves nothing: a bundle that throws on boot is a 200 and a blank page.
// Targets come from `.github/custom-app-checks.json`, passed in as JSON by
// `.github/workflows/custom-app-checks.yaml`.

type Target = { app: string; url: string; expectSelector?: string };
const raw = process.env.CUSTOM_APP_CHECK_TARGETS;
if (!raw)
  throw new Error("CUSTOM_APP_CHECK_TARGETS is not set: nothing to check is a failure, not a pass");
const targets: Target[] = JSON.parse(raw);
if (targets.length === 0) throw new Error("CUSTOM_APP_CHECK_TARGETS is empty");

const apiKey = process.env.OXY_API_KEY?.trim() || undefined;
// Origins that may receive the key besides each target's own: the environment's
// admin host, where the SDK sends `/sql/query` and `/query` when the app is
// served from a custom-app subdomain. Comma-separated; an unparseable entry
// throws here rather than silently dropping the key.
const extraKeyOrigins = (process.env.CUSTOM_APP_CHECK_ALLOWED_ORIGINS ?? "")
  .split(",")
  .map((o) => o.trim())
  .filter(Boolean)
  .map((o) => new URL(o).origin);

for (const t of targets) {
  test(`${t.app} loads and mounts`, async ({ context, page }) => {
    if (apiKey) {
      // The key goes only to origins that serve this app. `extraHTTPHeaders`
      // would put it on every request the page makes, a font CDN included.
      const keyOrigins = new Set([new URL(t.url).origin, ...extraKeyOrigins]);
      await context.route("**/*", (route) => {
        const request = route.request();
        if (!keyOrigins.has(new URL(request.url()).origin)) return route.continue();
        return route.continue({ headers: { ...request.headers(), "x-api-key": apiKey } });
      });
    }
    const pageErrors: string[] = [];
    page.on("pageerror", (e) => pageErrors.push(e.message));
    // The platform runtime pushes `oxy-app-ready` once the app has laid out
    // (custom_apps_client/runtime.js markReady) — a 200 shell for a missing or
    // crashed app never sends it. markReady flushes with fetch, not
    // sendBeacon, so the JSON batch (`{"v":1,"events":[{"n":"oxy-app-ready",…}]}`)
    // is readable as postData. It goes to `<app base>__oxy/beacon`, and the base
    // always ends in `/`.
    const ready = page.waitForRequest(
      (r) => r.url().includes("/__oxy/beacon") && (r.postData() ?? "").includes("oxy-app-ready"),
      { timeout: 30_000 }
    );
    await page.goto(t.url);
    await ready;
    if (t.expectSelector)
      await expect(page.locator(t.expectSelector)).toBeVisible({ timeout: 30_000 });
    expect(pageErrors, "uncaught page errors").toEqual([]);
  });
}
