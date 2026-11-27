/**
 * `oxyc checks run --build … --environment production --report`: the one
 * check of a production attempt. What is pinned: the claim names production,
 * the run waits until production's page reports the attempt's build, every
 * screen is opened as a reader with NO environment header and nothing signed
 * that staging's check would not sign, and the verdict is what the move broke
 * against the claim's baseline. Same stand-ins as `checks-build.test.ts`
 * (`testing/checks-run-harness.ts`).
 */

import { describe, expect, it } from "vitest";
import { list, useChecksRun } from "../testing/checks-run-harness.js";
import { BUILD_ID, BUILD_ROW, WORKSPACE_ID } from "../testing/fake-checks-platform.js";
import { FakeChromium, type PageScript } from "../testing/fake-playwright.js";
import { ExitCode } from "../util/errors.js";
import type { BuildChecksDeps } from "./checks-build.js";

const OLD_BUILD = "60e2f19a";
const OLD_ERROR = "TypeError: legacy is undefined";

describe("oxyc checks run --build --environment production", () => {
  const h = useChecksRun();

  /** Production serving `build` (a build's row id, as its page reports it): a root that links to `stores`. */
  function page(build: string, over: Partial<PageScript> = {}): PageScript {
    return {
      requests: [
        { url: `${h.platform.pageUrl}assets/index.js` },
        {
          method: "POST",
          url: `${h.platform.pageUrl}__oxy/beacon`,
          postData: '{"events":[{"n":"oxy-app-ready"}]}'
        }
      ],
      app: { environment: "production", buildId: build },
      links: [`${h.platform.pageUrl}stores`],
      ...over
    };
  }

  /** The claim of a production attempt: its one check, and nothing of staging's. */
  const claim = (overrides: Record<string, unknown> = {}) =>
    h.claim({
      environment: "production",
      attempt: 1,
      open: ["production_smoke"],
      screens: null,
      production: null,
      baseline: null,
      limits: { screens: 30, file_bytes: 2_000_000, files: 200 },
      ...overrides
    });

  const run = (chromium: FakeChromium, deps: Partial<BuildChecksDeps> = {}) =>
    h.run(chromium, deps, { attempt: "1", environment: "production" });

  it("claims the production attempt, and reports production_smoke for it", async () => {
    claim();
    const { summary, failure } = await run(new FakeChromium(() => page(BUILD_ROW)));
    expect(failure).toBeUndefined();
    expect(h.platform.routes()).toEqual(["POST /claim", "POST /report"]);
    expect(h.platform.bodies("POST /claim")).toEqual([
      { attempt: 1, runner: { label: "oxyc, run by hand", url: null }, environment: "production" }
    ]);
    expect(h.platform.bodies("POST /report")).toMatchObject([
      { attempt: 1, environment: "production", checks: [{ kind: "production_smoke" }] }
    ]);
    expect(h.reported()).toMatchObject({
      status: "passed",
      summary: "2 screens, no new errors",
      detail: {
        findings: [],
        baseline_at: null,
        discovered: true,
        screens_omitted: 0,
        new_errors: []
      }
    });
    expect(summary).toMatchObject({
      environment: "production",
      checks: [{ kind: "production_smoke", by: "runner", status: "passed", reported: true }]
    });
  });

  it("waits until production's page reports the attempt's build, re-reading it in a fresh context", async () => {
    claim();
    let reads = 0;
    // The first three reads still get the build before: a page cached somewhere on the way.
    const chromium = new FakeChromium((_, url) => {
      if (url === h.platform.pageUrl) reads += 1;
      return page(reads <= 3 ? OLD_BUILD : BUILD_ROW);
    });
    const { failure } = await run(chromium);
    expect(failure).toBeUndefined();
    // Asked again every five seconds, and judged only once it was the right build.
    expect(h.waits).toEqual([5000, 5000, 5000]);
    expect(h.reported()?.status).toBe("passed");
    // Four reads, each in its own context (0 to 3), then the two the screens are opened in.
    expect(chromium.contexts).toHaveLength(6);
    expect(chromium.closedContexts).toBe(6);
    // Every signed read asks for an uncached copy.
    expect(chromium.sent[0]?.headers).toMatchObject({ "cache-control": "no-cache" });
    const viewports = list<{ viewports: { build_id: string }[] }>(h.reported()?.detail.screens)
      .flatMap((s) => s.viewports)
      .map((v) => v.build_id);
    expect(viewports).toHaveLength(4);
    expect(new Set(viewports)).toEqual(new Set([BUILD_ROW]));
  });

  it("is an error when production never serves the build in time, and opens no screen", async () => {
    claim();
    const chromium = new FakeChromium(() => page(OLD_BUILD));
    const { failure } = await run(chromium, { productionWaitMs: 30 });
    expect(h.reported()).toMatchObject({
      status: "error",
      summary: `production did not serve build ${BUILD_ID} within 0s (its page reports build ${OLD_BUILD}, not ${BUILD_ROW})`,
      detail: { served_build_id: OLD_BUILD, screens: [], new_errors: [] }
    });
    // Only the root was ever read, at desk width.
    expect(new Set(chromium.contexts.map((c) => c.viewport?.width))).toEqual(new Set([1440]));
    expect(chromium.sent.some((s) => s.url.endsWith("/stores"))).toBe(false);
    // No verdict is an answer of "no" for the run, not a broken one.
    expect(failure?.code).toBe(ExitCode.CHECK_FAILED);
  });

  it("sends no environment header, signs only what staging's check signs, and sends no write", async () => {
    claim();
    const chromium = new FakeChromium(() =>
      page(BUILD_ROW, {
        requests: [
          { method: "POST", url: `${h.platform.url}/api/projects/${WORKSPACE_ID}/query` },
          { method: "POST", url: `${h.platform.pageUrl}fn/save` },
          { url: `${h.platform.url}/api/orgs`, status: 401 },
          { url: "https://fonts.example.com/a.woff2" },
          ...(page(BUILD_ROW).requests ?? [])
        ]
      })
    );
    await run(chromium);
    const signed = chromium.sent.filter((s) => s.headers["x-api-key"] !== undefined);
    expect(signed.length).toBeGreaterThan(0);
    for (const sent of chromium.sent) {
      // Production is what the URL serves unnamed: the header is on no request at all.
      expect(Object.keys(sent.headers), sent.url).not.toContain("x-oxy-app-env");
    }
    // Signed: the document, its assets, the beacon and the app's own data read. Nothing else.
    const kinds = new Set(
      signed.map((s) => `${s.method} ${s.url.replace(h.platform.url, "").replace(/stores$/, "")}`)
    );
    expect(kinds).toEqual(
      new Set([
        "GET /customer-apps/acme/store/",
        "GET /customer-apps/acme/store/assets/index.js",
        "POST /customer-apps/acme/store/__oxy/beacon",
        `POST /api/projects/${WORKSPACE_ID}/query`
      ])
    );
    // The write is refused in the browser, never sent, and only warned about.
    const writes = chromium.sent.filter((s) => s.url.endsWith("fn/save"));
    expect(writes.length).toBeGreaterThan(0);
    expect(new Set(writes.map((s) => s.decision))).toEqual(new Set(["aborted"]));
    expect(h.reported()?.status).toBe("passed");
    expect(h.reported()?.detail.findings[0]).toMatchObject({
      severity: "warning",
      message: "/ at desk: a request answered 401"
    });
    // Another origin gets nothing at all, on production as on staging: refused, and warned about.
    const fonts = chromium.sent.filter((s) => s.url.startsWith("https://fonts.example.com/"));
    expect(fonts.length).toBeGreaterThan(0);
    expect(new Set(fonts.map((s) => s.decision))).toEqual(new Set(["aborted"]));
    expect(h.reported()?.detail.findings).toContainEqual({
      severity: "warning",
      message:
        "/ at desk: not sent: GET https://fonts.example.com/a.woff2 (another origin) — a checked page reaches only the app's own origin",
      at: null,
      rule: "off-origin-refused"
    });
  });

  it("holds the screens to the baseline: an error it had is not new, a screen it had ready and now is not, is", async () => {
    claim({
      baseline: {
        at: "2026-11-22T10:00:00Z",
        screens: [
          { path: "", viewport: "desk", ready: true, errors: [OLD_ERROR] },
          { path: "", viewport: "phone", ready: true, errors: [OLD_ERROR] },
          { path: "stores", viewport: "desk", ready: true, errors: [] },
          { path: "stores", viewport: "phone", ready: true, errors: [] }
        ]
      }
    });
    const chromium = new FakeChromium((context, url) => {
      const stores = url.endsWith("/stores");
      // The root still throws what it threw before the move; `stores` no longer mounts on a phone.
      if (!stores) return page(BUILD_ROW, { errors: [new TypeError("legacy is undefined")] });
      const phone = chromium.contexts[context]?.viewport?.width === 390;
      return page(BUILD_ROW, phone ? { requests: [] } : {});
    });
    const { failure } = await run(chromium);
    expect(h.reported()).toMatchObject({
      status: "failed",
      summary: "1 new error on 1 of 2 screens",
      detail: {
        baseline_at: "2026-11-22T10:00:00Z",
        new_errors: [
          {
            path: "stores",
            viewport: "phone",
            error:
              "the app never sent oxy-app-ready within 0s — a blank page, a crash on boot, or a sign-in page"
          }
        ]
      }
    });
    expect(h.reported()?.detail.findings).toHaveLength(1);
    expect(failure?.code).toBe(ExitCode.CHECK_FAILED);
    expect(failure?.message).toBe(
      `1 of 1 check(s) did not pass on build ${BUILD_ID}: Production opens after promote`
    );
  });

  it("calls no error new when there is no baseline: it skips, and fails nothing", async () => {
    // A promoting publish: staging and production moved together, so `changes` never ran.
    claim({ baseline: null });
    const chromium = new FakeChromium(() =>
      page(BUILD_ROW, { errors: [new TypeError("legacy is undefined")] })
    );
    const { failure } = await run(chromium);
    expect(h.reported()).toMatchObject({
      status: "skipped",
      summary:
        "4 errors on 2 of 2 screens, and no baseline from before the move to tell new ones apart",
      detail: { baseline_at: null, new_errors: [] }
    });
    const findings = list<{ severity: string; message: string }>(h.reported()?.detail.findings);
    expect(findings.filter((f) => f.severity === "warning").map((f) => f.message)).toContain(
      `/ at desk: ${OLD_ERROR} (no baseline: not called new)`
    );
    expect(findings.some((f) => f.severity === "error")).toBe(false);
    expect(failure).toBeUndefined();
  });

  it("calls no error new on a screen the baseline never saw: it skips, and fails nothing", async () => {
    // The comparison ran out of time before `stores`: the baseline lists the root alone.
    claim({
      baseline: {
        at: "2026-11-22T10:00:00Z",
        screens: [
          { path: "", viewport: "desk", ready: true, errors: [OLD_ERROR] },
          { path: "", viewport: "phone", ready: true, errors: [OLD_ERROR] }
        ]
      }
    });
    // Every screen throws what production threw before the move.
    const chromium = new FakeChromium(() =>
      page(BUILD_ROW, { errors: [new TypeError("legacy is undefined")] })
    );
    const { failure } = await run(chromium);
    expect(h.reported()).toMatchObject({
      status: "skipped",
      summary:
        "2 errors on 1 of 2 screens the baseline from before the move cannot tell apart: not called new",
      detail: { baseline_at: "2026-11-22T10:00:00Z", new_errors: [] }
    });
    const findings = list<{ severity: string; message: string }>(h.reported()?.detail.findings);
    expect(findings.map((f) => [f.severity, f.message])).toEqual([
      ["warning", `/stores at desk: ${OLD_ERROR} (not in the baseline: not called new)`],
      ["warning", `/stores at phone: ${OLD_ERROR} (not in the baseline: not called new)`]
    ]);
    // Nothing failed: no notice, and nothing for a rollback to act on.
    expect(failure).toBeUndefined();
  });

  it("opens the build's declared screens when the claim hands them over", async () => {
    claim({ screens: ["stores"] });
    const chromium = new FakeChromium(() => page(BUILD_ROW));
    await run(chromium);
    expect(h.reported()).toMatchObject({
      summary: "1 screen, no new errors",
      detail: { discovered: false }
    });
  });

  it("refuses to run when the platform answers a staging attempt instead", async () => {
    claim({ environment: "staging", open: ["functions", "loads", "design_system"] });
    const chromium = new FakeChromium(() => page(BUILD_ROW));
    const { failure } = await run(chromium);
    expect(failure?.code).toBe(ExitCode.FAILURE);
    expect(failure?.message).toBe(
      "the platform answered a staging attempt, not the production one this run asked for"
    );
    expect(h.platform.routes()).toEqual(["POST /claim"]);
    expect(chromium.launches).toBe(0);
  });

  it("takes no picture and uploads no file: production's check keeps nothing a page showed", async () => {
    claim();
    const chromium = new FakeChromium(() => page(BUILD_ROW, { text: "a customer's numbers" }));
    await run(chromium);
    expect(chromium.shots).toEqual([]);
    expect(h.platform.files()).toEqual([]);
    expect(JSON.stringify(h.platform.bodies("POST /report"))).not.toContain("customer's numbers");
  });
});
