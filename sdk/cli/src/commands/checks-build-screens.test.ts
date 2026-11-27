/**
 * `oxyc checks run --build … --report` on an attempt that has `smoke` open:
 * which screens are opened, through which path, and when and as what the
 * check is reported. Same stand-ins as `checks-build.test.ts`: a platform on
 * a loopback port and a scripted browser (`testing/checks-run-harness.ts`,
 * `testing/screens-app.ts`).
 *
 * What `changes` keeps of those screens: `checks-build-changes.test.ts`.
 */

import { describe, expect, it } from "vitest";
import { confinementArgs } from "../checks/browser-confine.js";
import { list, type Reported, useChecksRun } from "../testing/checks-run-harness.js";
import { BUILD_ROW, claimOf } from "../testing/fake-checks-platform.js";
import { FakeChromium } from "../testing/fake-playwright.js";
import { GATE_CLAIM, onProduction, screensApp } from "../testing/screens-app.js";
import type { BuildChecksDeps, BuildChecksSummary } from "./checks-build.js";

describe("oxyc checks run --build: every screen opens", () => {
  const h = useChecksRun();
  const app = (context: number, url: string) => screensApp(h.platform)(context, url);
  const claim = (overrides: Record<string, unknown> = {}) =>
    h.claim({ ...GATE_CLAIM, ...overrides });
  const reported = (kind: string) => h.reported(kind);
  const run = async (chromium: FakeChromium, deps: Partial<BuildChecksDeps> = {}) =>
    (await h.run(chromium, deps)).summary as BuildChecksSummary;

  it("finds the screens in the root's links and opens each at both widths, through the guarded path", async () => {
    claim();
    const chromium = new FakeChromium(app);
    await run(chromium);

    // One browser for both checks, sandboxed and held to the deployment's own origin; a fresh
    // context per viewport and environment.
    expect(chromium.launchOptions).toEqual([
      { headless: true, chromiumSandbox: true, args: confinementArgs(h.platform.url), env: {} }
    ]);
    expect(chromium.contexts.map((c) => c.viewport?.width)).toEqual([
      1440,
      390,
      undefined,
      1440,
      390
    ]);
    expect(chromium.browserClosed).toBe(true);

    // Staging at desk width: the root first, then each same-app link once. Nothing off the app.
    const opened = (context: number) =>
      chromium.sent
        .filter((s) => s.context === context && s.method === "GET")
        .map((s) => s.url.slice(h.platform.pageUrl.length));
    expect(opened(0)).toEqual(["", "stores", "reorder/plans"]);
    expect(opened(1)).toEqual(["", "stores", "reorder/plans"]);
    expect(opened(3)).toEqual(["", "stores", "reorder/plans"]);

    const smoke = reported("smoke");
    expect(smoke).toMatchObject({
      status: "passed",
      summary: "3 screens, 0 errors",
      detail: { findings: [], discovered: true, screens_omitted: 0 }
    });
    // Every linked screen is in the list, so none is named as left unopened.
    expect(smoke?.detail.screens_unlisted).toBeUndefined();
    const screens = list<{ path: string; viewports: { name: string }[] }>(smoke?.detail.screens);
    expect(screens.map((s) => [s.path, s.viewports.map((v) => v.name)])).toEqual([
      ["", ["desk", "phone"]],
      ["stores", ["desk", "phone"]],
      ["reorder/plans", ["desk", "phone"]]
    ]);
    expect(screens[1]?.viewports[1]).toEqual({
      name: "phone",
      width: 390,
      height: 844,
      ready: true,
      ready_ms: expect.any(Number),
      environment: "staging",
      build_id: BUILD_ROW,
      page_errors: [],
      failed_requests: [],
      refused_writes: [],
      refused_off_origin: []
    });

    // Staging is asked for by name; production by no name at all. The credential is the same.
    for (const sent of chromium.sent) {
      expect(sent.headers["x-api-key"], sent.url).toBe("oxysvc_checker");
      expect(sent.headers["x-oxy-app-env"], sent.url).toBe(
        onProduction(sent.context) ? undefined : "staging"
      );
    }
  });

  it("reports smoke as soon as staging has been opened, before production is", async () => {
    claim();
    const chromium = new FakeChromium(app);
    const productionRequestsAt: Record<string, number> = {};
    h.platform.on("POST /report", (call) => {
      const kind = (call.body as { checks: Reported[] }).checks[0]?.kind ?? "?";
      productionRequestsAt[kind] = chromium.sent.filter((s) => onProduction(s.context)).length;
      return { status: 200, body: { state: "running", open: [] } };
    });
    const summary = await run(chromium);
    expect(productionRequestsAt.smoke).toBe(0);
    expect(productionRequestsAt.changes).toBeGreaterThan(0);
    expect(summary.checks.map((c) => [c.kind, c.status, c.reported])).toEqual([
      ["smoke", "passed", true],
      ["changes", "passed", true]
    ]);
    // Each report names the attempt and its environment.
    expect(h.platform.bodies("POST /report")).toMatchObject([
      { attempt: 2, environment: "staging", checks: [{ kind: "smoke" }] },
      { attempt: 2, environment: "staging", checks: [{ kind: "changes" }] }
    ]);
  });

  it("opens the screens the build declares, in its order, and names a linked screen it did not open", async () => {
    claim({ screens: ["stores", "", "fn/save", "../other-app/"] });
    const chromium = new FakeChromium(app);
    await run(chromium);
    const smoke = reported("smoke");
    expect(smoke?.detail).toMatchObject({ discovered: false, screens_omitted: 0 });
    // Both screens link to `reorder/plans`, which the build did not declare: named once, never opened.
    expect(smoke?.detail).toMatchObject({
      screens_unlisted: ["reorder/plans"],
      screens_unlisted_total: 1
    });
    expect(chromium.sent.some((s) => s.url.includes("reorder/plans"))).toBe(false);
    expect(list<{ path: string }>(smoke?.detail.screens).map((s) => s.path)).toEqual([
      "stores",
      ""
    ]);
    expect(reported("changes")?.summary).toBe("2 of 4 screens differ");
    // No function route is ever opened, declared or not.
    expect(chromium.sent.some((s) => s.url.includes("/fn/"))).toBe(false);
  });

  it("opens a declared screen with its query, on staging and on production, under that name", async () => {
    claim({ screens: ["", "?module=waste", "stores?tab=open"] });
    const chromium = new FakeChromium(app);
    await run(chromium);
    const opened = (context: number) =>
      chromium.sent
        .filter((s) => s.context === context && s.method === "GET")
        .map((s) => s.url.slice(h.platform.pageUrl.length));
    for (const context of [0, 1, 3, 4]) {
      expect(opened(context), `context ${context}`).toEqual([
        "",
        "?module=waste",
        "stores?tab=open"
      ]);
    }
    const smoke = reported("smoke");
    expect(smoke).toMatchObject({ status: "passed", summary: "3 screens, 0 errors" });
    expect(list<{ path: string }>(smoke?.detail.screens).map((s) => s.path)).toEqual([
      "",
      "?module=waste",
      "stores?tab=open"
    ]);
    // Each screen is compared under its own name, the query's words in its files' names.
    const compared = list<{ path: string; staging: string }>(reported("changes")?.detail.screens);
    expect(compared.filter((s) => s.path === "?module=waste").map((s) => s.staging)).toEqual([
      "shot-staging-module-waste-desk.png",
      "shot-staging-module-waste-phone.png"
    ]);
  });

  it("does not call a screen undeclared when the cap, not the list, left it out", async () => {
    // Two declared, one opened: `stores` is left out by the cap, and every page links to it.
    claim({
      screens: ["", "stores"],
      limits: { ...GATE_CLAIM.limits, screens: 1 }
    });
    await run(new FakeChromium(app));
    const smoke = reported("smoke");
    expect(smoke?.detail).toMatchObject({ discovered: false, screens_omitted: 1 });
    // `reorder/plans` was never declared; `stores` was, and is not named beside it.
    expect(smoke?.detail).toMatchObject({
      screens_unlisted: ["reorder/plans"],
      screens_unlisted_total: 1
    });
  });

  it("says how far it got when the time runs out, and never calls an unopened screen fine", async () => {
    claim({ screens: ["", "stores"] });
    await run(new FakeChromium(app), { screensBudgetMs: 0 });
    expect(reported("smoke")).toMatchObject({
      status: "error",
      summary: "0 of 2 screens opened before the time was up",
      detail: { screens: [], screens_unvisited: 2 }
    });
    expect(reported("changes")).toMatchObject({
      status: "error",
      summary: "no screen could be compared before the time was up"
    });
  });

  it("fails smoke on a screen that throws at one width, and still compares", async () => {
    claim();
    const chromium = new FakeChromium((context, url) => ({
      ...app(context, url),
      errors:
        context === 1 && url.endsWith("/stores") ? [new TypeError("rows is undefined")] : undefined
    }));
    const summary = await run(chromium);
    expect(reported("smoke")).toMatchObject({
      status: "failed",
      summary: "1 of 3 screens has errors"
    });
    expect(reported("smoke")?.detail.findings[0]?.message).toBe(
      "/stores at phone: uncaught page error: TypeError: rows is undefined"
    );
    expect(reported("changes")?.status).toBe("passed");
    expect(summary.checks.map((c) => c.status)).toEqual(["failed", "passed"]);
  });

  it("reports both as error, with why, when the browser cannot start", async () => {
    claim();
    const chromium = new FakeChromium(app);
    chromium.launchError = "browserType.launch: No usable sandbox!";
    await run(chromium);
    for (const kind of ["smoke", "changes"]) {
      expect(reported(kind), kind).toMatchObject({
        status: "error",
        summary:
          "Chromium could not start with its sandbox on (no usable sandbox on this machine); the page is not opened without one"
      });
    }
  });

  it("reports both as error when the platform names no page to open", async () => {
    claim({ url: null });
    const chromium = new FakeChromium(app);
    await run(chromium);
    for (const kind of ["smoke", "changes"]) {
      expect(reported(kind), kind).toMatchObject({
        status: "error",
        summary: "the platform named no page to open for this build"
      });
    }
    expect(chromium.launches).toBe(0);
  });

  it("runs neither where the attempt has neither: an org with no gate is checked as before", async () => {
    // The claim a deployment answers with the gate off: the four first checks, no screens, no limits.
    h.platform.on("POST /claim", () => ({
      status: 200,
      body: claimOf(h.platform.pageUrl, { open: ["design_system"] })
    }));
    const chromium = new FakeChromium(app);
    const summary = await run(chromium);
    expect(h.platform.routes()).toEqual(["POST /claim", "GET /stylesheets", "POST /report"]);
    expect(chromium.launches).toBe(0);
    expect(summary.checks.map((c) => c.kind)).toEqual(["design_system"]);
  });
});
