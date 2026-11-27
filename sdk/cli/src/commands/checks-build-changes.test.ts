/**
 * `oxyc checks run --build … --report` on an attempt that has `changes` open:
 * each screen pictured on staging and on production, the comparison, the
 * files kept on the platform and under which names, and what a limit or a
 * failed upload does. The pictures are a few scripted pixels, compared by the
 * check's own comparison (`testing/fake-page.ts`).
 *
 * Which screens are opened, and `smoke`: `checks-build-screens.test.ts`.
 */

import { describe, expect, it } from "vitest";
import { list, useChecksRun } from "../testing/checks-run-harness.js";
import { readFakePng } from "../testing/fake-page.js";
import { FakeChromium } from "../testing/fake-playwright.js";
import { GATE_CLAIM, PRODUCTION_BUILD, SECRET, screensApp } from "../testing/screens-app.js";
import type { BuildChecksDeps, BuildChecksSummary } from "./checks-build.js";

describe("oxyc checks run --build: what changed", () => {
  const h = useChecksRun();
  const app = (context: number, url: string) => screensApp(h.platform)(context, url);
  const claim = (overrides: Record<string, unknown> = {}) =>
    h.claim({ ...GATE_CLAIM, ...overrides });
  const reported = (kind: string) => h.reported(kind);
  const run = async (chromium: FakeChromium, deps: Partial<BuildChecksDeps> = {}) =>
    (await h.run(chromium, deps)).summary as BuildChecksSummary;

  it("pictures each screen on both environments and compares them", async () => {
    claim();
    const chromium = new FakeChromium(app);
    await run(chromium);

    const row = (path: string, viewport: string, slug: string, changed: boolean) => ({
      path,
      viewport,
      changed,
      ratio: changed ? 0.5 : 0,
      staging: `shot-staging-${slug}-${viewport}.png`,
      production: `shot-production-${slug}-${viewport}.png`,
      diff: changed ? `diff-${slug}-${viewport}.png` : null,
      production_ready: true,
      production_errors: []
    });
    expect(reported("changes")).toEqual({
      kind: "changes",
      status: "passed",
      summary: "2 of 6 screens differ",
      detail: {
        findings: [],
        production_build_id: PRODUCTION_BUILD,
        compared: 6,
        changed: 2,
        screens: [
          row("", "desk", "root", false),
          row("", "phone", "root", false),
          row("stores", "desk", "stores", true),
          row("stores", "phone", "stores", true),
          row("reorder/plans", "desk", "reorder--plans", false),
          row("reorder/plans", "phone", "reorder--plans", false)
        ],
        seen: null
      }
    });

    // Every picture is the full page cut at 4,000 px, one pixel per CSS pixel, animations off.
    expect(chromium.shots).toHaveLength(12);
    expect(chromium.shots[0]?.options).toEqual({
      type: "png",
      fullPage: true,
      clip: { x: 0, y: 0, width: 2880, height: 4000 },
      animations: "disabled",
      caret: "hide",
      scale: "css",
      timeout: 15_000
    });
    expect(chromium.compared).toBe(6);
  });

  it("keeps the evidence of that run under the contract's names", async () => {
    claim();
    await run(new FakeChromium(app));

    // Two pictures per screen and viewport; a diff and both texts only where it changed; the digest once.
    const both = (name: (viewport: string) => string) => ["desk", "phone"].map(name);
    expect(h.fileNames()).toEqual(
      [
        "css-digest.json",
        ...["root", "stores", "reorder--plans"].flatMap((slug) =>
          both((v) => `shot-staging-${slug}-${v}.png`).concat(
            both((v) => `shot-production-${slug}-${v}.png`)
          )
        ),
        ...both((v) => `diff-stores-${v}.png`),
        ...both((v) => `text-staging-stores-${v}.json`),
        ...both((v) => `text-production-stores-${v}.json`)
      ].sort()
    );
    for (const file of h.platform.files()) {
      expect([file.attempt, file.environment], file.name).toEqual(["2", "staging"]);
      expect(file.contentType, file.name).toBe(
        file.name.endsWith(".png") ? "image/png" : "application/json"
      );
    }
    const file = (name: string) => h.platform.files().find((f) => f.name === name)?.bytes as Buffer;
    expect(JSON.parse(file("text-staging-stores-desk.json").toString("utf8"))).toEqual({
      text: `stores · ${SECRET}`
    });
    // The diff is the compared area, marked where the two pictures differ.
    const diff = readFakePng(file("diff-stores-phone.png"));
    expect([diff.width, diff.height]).toEqual([4, 4]);
    expect(diff.data.slice(0, 4)).toEqual([219, 39, 119, 255]);
    // The digest is made from the stylesheets the platform hands out, read once.
    expect(JSON.parse(file("css-digest.json").toString("utf8"))).toEqual({ rules: [], omitted: 0 });
    expect(h.platform.routes().filter((r) => r === "GET /stylesheets")).toHaveLength(1);
  });

  it("prints no page text and no picture to the job log", async () => {
    claim();
    await run(new FakeChromium(app));
    for (const stream of ["stdout", "stderr"] as const) {
      expect(h.printed(stream)).not.toContain("payroll");
      expect(h.printed(stream)).not.toContain('"data":[');
    }
    // The report keeps none of it either: names and counts only.
    expect(JSON.stringify(h.platform.bodies("POST /report"))).not.toContain("payroll");
  });

  it("skips changes when production serves nothing else: no picture is taken, the digest is still kept", async () => {
    claim({ production: null });
    const chromium = new FakeChromium(app);
    const summary = await run(chromium);
    expect(reported("changes")).toEqual({
      kind: "changes",
      status: "skipped",
      summary: "nothing to compare: production serves no other build",
      detail: {
        findings: [],
        production_build_id: null,
        compared: 0,
        changed: 0,
        screens: [],
        seen: null
      }
    });
    expect(reported("smoke")?.status).toBe("passed");
    expect(chromium.shots).toEqual([]);
    expect(chromium.contexts).toHaveLength(2);
    expect(h.fileNames()).toEqual(["css-digest.json"]);
    // The digest goes up before the check gets its verdict: a closed attempt takes no file.
    const routes = h.platform.routes();
    expect(routes.indexOf("PUT /files")).toBeLessThan(routes.indexOf("POST /report"));
    expect(summary.checks.map((c) => [c.kind, c.status])).toEqual([
      ["smoke", "passed"],
      ["changes", "skipped"]
    ]);
  });

  it("makes a failed upload a warning, not a failed check", async () => {
    claim();
    h.platform.on("PUT /files", (call) =>
      call.file?.name === "shot-production-stores-desk.png"
        ? { status: 500, body: { code: "store_unavailable", message: "try again" } }
        : { status: 204 }
    );
    await run(new FakeChromium(app));
    const changes = reported("changes");
    expect(changes).toMatchObject({ status: "passed", summary: "2 of 6 screens differ" });
    const rows = list<{ path: string; viewport: string }>(changes?.detail.screens);
    expect(rows.find((r) => r.path === "stores" && r.viewport === "desk")).toMatchObject({
      changed: true,
      staging: "shot-staging-stores-desk.png",
      // Not kept, so not named.
      production: null,
      diff: "diff-stores-desk.png"
    });
    expect(changes?.detail.findings).toHaveLength(1);
    expect(changes?.detail.findings[0]).toMatchObject({ severity: "warning" });
    expect(changes?.detail.findings[0]?.message).toMatch(
      /^shot-production-stores-desk\.png could not be uploaded: 500 /
    );
    // Sent again once, and no more.
    expect(
      h.platform.calls.filter((c) => c.file?.name === "shot-production-stores-desk.png")
    ).toHaveLength(2);
    expect(h.fileNames()).not.toContain("shot-production-stores-desk.png");
  });

  it("counts a screen that could not be pictured as changed, and says why", async () => {
    claim();
    // Production's `stores` cannot be pictured at desk width.
    const chromium = new FakeChromium((context, url) => ({
      ...app(context, url),
      ...(context === 3 && url.endsWith("/stores") ? { shot: null } : {})
    }));
    await run(chromium);
    const changes = reported("changes");
    expect(changes).toMatchObject({ status: "passed", summary: "2 of 6 screens differ" });
    const rows = list<{ path: string; viewport: string }>(changes?.detail.screens);
    expect(rows.find((r) => r.path === "stores" && r.viewport === "desk")).toEqual({
      path: "stores",
      viewport: "desk",
      changed: true,
      ratio: 1,
      staging: "shot-staging-stores-desk.png",
      production: null,
      diff: null,
      production_ready: true,
      production_errors: []
    });
    expect(changes?.detail.findings).toEqual([
      {
        severity: "warning",
        message:
          "/stores at desk: no picture of production could be taken, so the screen counts as changed",
        at: null,
        rule: null
      }
    ]);
    // Nothing was compared for it, and no text is kept without a picture taken.
    expect(chromium.compared).toBe(5);
    expect(h.fileNames()).not.toContain("text-production-stores-desk.json");
    expect(h.fileNames()).toContain("text-staging-stores-desk.json");
  });

  it("gives up on uploads that keep failing, and still reports what it compared", async () => {
    claim();
    h.platform.on("PUT /files", () => ({
      status: 503,
      body: { code: "store_unavailable", message: "try again" }
    }));
    await run(new FakeChromium(app));
    const changes = reported("changes");
    expect(changes).toMatchObject({
      status: "passed",
      summary: "2 of 6 screens differ",
      detail: { compared: 6, changed: 2 }
    });
    expect(h.fileNames()).toEqual([]);
    // Each file is sent twice at most, and after three files in a row no more are tried:
    // the digest, then at most two per viewport before both have heard.
    const puts = h.platform.calls.filter((c) => c.route === "PUT /files").length;
    expect(puts).toBeGreaterThanOrEqual(6);
    expect(puts).toBeLessThanOrEqual(10);
    expect(changes?.detail.findings.at(-1)?.message).toMatch(
      /^3 uploads failed in a row: no more were tried \(\d+ more file\(s\) were not uploaded\)$/
    );
    for (const row of list<{ staging: unknown; production: unknown }>(changes?.detail.screens)) {
      expect([row.staging, row.production]).toEqual([null, null]);
    }
  });

  it("keeps no file larger than the claim allows, and never sends it", async () => {
    // The digest is 24 bytes and a text about 60; a picture is some 250.
    claim({ limits: { screens: 30, file_bytes: 120, files: 200 } });
    await run(new FakeChromium(app));
    const changes = reported("changes");
    expect(changes?.status).toBe("passed");
    expect(h.fileNames().some((name) => name.endsWith(".png"))).toBe(false);
    expect(h.platform.calls.some((c) => c.file?.name.endsWith(".png"))).toBe(false);
    expect(h.fileNames()).toContain("text-staging-stores-desk.json");
    expect(changes?.detail.findings[0]?.message).toMatch(
      /^shot-staging-root-desk\.png is \d+ bytes, more than the 120 a file may be: not uploaded$/
    );
    // The comparison still ran: the pictures were too large to keep, not to compare.
    expect(changes?.summary).toBe("2 of 6 screens differ");
  });

  it("stops at the attempt's file count, and at the claim's screen count", async () => {
    claim({ limits: { screens: 2, file_bytes: 2_000_000, files: 4 } });
    const chromium = new FakeChromium(app);
    await run(chromium);
    expect(reported("smoke")).toMatchObject({
      summary: "2 screens, 0 errors",
      detail: { discovered: true, screens_omitted: 1 }
    });
    expect(h.platform.files()).toHaveLength(4);
    expect(h.platform.calls.filter((c) => c.route === "PUT /files")).toHaveLength(4);
    const changes = reported("changes");
    expect(changes).toMatchObject({ status: "passed", detail: { compared: 4 } });
    const last = changes?.detail.findings.at(-1)?.message ?? "";
    expect(last).toMatch(
      /^the attempt's 4 files are used: \S+ was not uploaded \(\d+ more file\(s\) were not uploaded\)$/
    );
  });

  it("stops asking once the platform says the attempt holds all it may", async () => {
    claim();
    h.platform.on("PUT /files", (_, nth) =>
      nth <= 3
        ? { status: 204 }
        : { status: 409, body: { code: "too_many_files", message: "200 files" } }
    );
    await run(new FakeChromium(app));
    // The refusal is believed: it is not sent again, and nothing after it is tried. The two
    // viewports upload side by side, so one more may already be on its way when it lands.
    const puts = h.platform.calls.filter((c) => c.route === "PUT /files").length;
    expect(puts).toBeGreaterThanOrEqual(4);
    expect(puts).toBeLessThanOrEqual(5);
    expect(h.platform.files()).toHaveLength(3);
    const messages = reported("changes")?.detail.findings.map((f) => f.message) ?? [];
    expect(messages.at(-1)).toMatch(/^the attempt holds all the files it may: /);
    expect(reported("changes")?.status).toBe("passed");
  });
});
