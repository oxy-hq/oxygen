/**
 * The verdicts over every screen: `smoke` (each rule that fails a screen, and
 * what only warns), and `production_smoke` (what counts as broken by the
 * move, against the baseline). Pure scoring over page loads written by hand.
 */

import { describe, expect, it } from "vitest";
import { newObserved, type Observed } from "./loads-score.js";
import { loadsTarget, VIEWPORTS } from "./loads-target.js";
import type { Baseline } from "./promote-api.js";
import type { ScreensPass, ScreenVisit } from "./screens-browser.js";
import { scoreProductionSmoke, scoreSmoke } from "./smoke-score.js";

const ORIGIN = "https://app.example.com";
const [DESK, PHONE] = VIEWPORTS;

function target(environment: string, build = "b1") {
  return loadsTarget({
    pageUrl: `${ORIGIN}/customer-apps/acme/store/`,
    deployment: ORIGIN,
    credential: { "x-api-key": "oxysvc_x" },
    appId: "00000000-0000-0000-0000-0000000000a1",
    environment,
    expectedBuild: build
  });
}

/** A load that became ready on the build being checked, changed by `over`. */
function seen(environment: string, over: Partial<Observed> = {}): Observed {
  return { ...newObserved(), readyMs: 900, app: { environment, buildId: "b1" }, ...over };
}

/** Every path at both viewports, each load `fine` unless `loads` says otherwise (`"stores@desk"`). */
function pass(
  environment: string,
  paths: string[],
  loads: Record<string, Partial<Observed>> = {},
  list: Partial<ScreensPass["list"]> = {}
): ScreensPass {
  const visits: ScreenVisit[] = paths.flatMap((path) =>
    [DESK, PHONE].map((viewport) => ({
      path,
      viewport,
      observed: seen(environment, loads[`${path}@${viewport.name}`])
    }))
  );
  return {
    list: { paths, discovered: true, omitted: 0, ...list },
    visits,
    unvisited: 0,
    unlisted: { paths: [], total: 0 }
  };
}

const viewport = (name: string, over: Record<string, unknown> = {}) => ({
  name,
  width: name === "desk" ? 1440 : 390,
  height: name === "desk" ? 900 : 844,
  ready: true,
  ready_ms: 900,
  environment: "staging",
  build_id: "b1",
  page_errors: [],
  failed_requests: [],
  refused_writes: [],
  refused_off_origin: [],
  ...over
});

describe("scoreSmoke", () => {
  const staging = target("staging");

  it("passes when every screen at every viewport is ready and threw nothing, in the contract's shape", () => {
    const outcome = scoreSmoke(staging, pass("staging", ["", "stores"], {}, { omitted: 3 }));
    expect(outcome).toEqual({
      status: "passed",
      summary: "2 screens, 0 errors",
      detail: {
        findings: [],
        discovered: true,
        screens_omitted: 3,
        screens: [
          { path: "", viewports: [viewport("desk"), viewport("phone")] },
          { path: "stores", viewports: [viewport("desk"), viewport("phone")] }
        ]
      }
    });
    expect(scoreSmoke(staging, pass("staging", [""])).summary).toBe("1 screen, 0 errors");
  });

  it.each([
    ["a navigation error", { navigationError: "the page answered 500" }, "the page answered 500"],
    [
      "the wrong environment",
      { app: { environment: "production", buildId: "b1" } },
      "the page was served by production, not staging"
    ],
    [
      "the wrong build",
      { app: { environment: "staging", buildId: "b0" } },
      "the page was served build b0, but staging serves b1"
    ],
    ["no ready beacon", { readyMs: null }, "the app never sent oxy-app-ready within 30s"],
    [
      "an uncaught page error",
      { pageErrors: ["TypeError: x is undefined"] },
      "uncaught page error: TypeError: x is undefined"
    ]
  ] as const)("fails a screen with %s, at one viewport of one screen", (_, wrong, why) => {
    const outcome = scoreSmoke(
      staging,
      pass("staging", ["", "stores", "plans"], { "stores@phone": wrong as Partial<Observed> })
    );
    expect(outcome.status).toBe("failed");
    expect(outcome.summary).toBe("1 of 3 screens has errors");
    expect(outcome.detail.findings).toHaveLength(1);
    expect(outcome.detail.findings[0]).toMatchObject({ severity: "error" });
    expect(outcome.detail.findings[0]?.message.startsWith(`/stores at phone: ${why}`)).toBe(true);
  });

  it("names the screens it saw linked and did not open, without changing the verdict", () => {
    const linked = { ...pass("staging", ["", "stores"]), unlisted: { paths: ["plans"], total: 4 } };
    const outcome = scoreSmoke(staging, linked);
    expect(outcome.status).toBe("passed");
    expect(outcome.summary).toBe("2 screens, 0 errors");
    expect(outcome.detail.findings).toEqual([]);
    expect(outcome.detail).toMatchObject({
      screens_unlisted: ["plans"],
      screens_unlisted_total: 4
    });
    // With none there is no key at all: a reader tells "none" from "an older runner" by the list.
    const none = scoreSmoke(staging, pass("staging", [""])).detail;
    expect("screens_unlisted" in none || "screens_unlisted_total" in none).toBe(false);
  });

  it("counts a screen once however many of its viewports fail, and says how many of how many", () => {
    const broken = { readyMs: null };
    const outcome = scoreSmoke(
      staging,
      pass("staging", ["", "stores", "plans"], {
        "@desk": broken,
        "@phone": broken,
        "plans@desk": broken
      })
    );
    expect(outcome.summary).toBe("2 of 3 screens have errors");
    expect(outcome.detail.findings.map((f) => f.message.split(":")[0])).toEqual([
      "/ at desk",
      "/ at phone",
      "/plans at desk"
    ]);
  });

  it("only warns about a failed request and a write the check refused", () => {
    const outcome = scoreSmoke(
      staging,
      pass("staging", ["stores"], {
        "stores@desk": {
          failedRequests: [{ url: `${ORIGIN}/api/x`, status: 403, error: null }],
          refused: ["POST /customer-apps/acme/store/fn/save"]
        }
      })
    );
    expect(outcome.status).toBe("passed");
    expect(outcome.summary).toBe("1 screen, 0 errors");
    expect(outcome.detail.findings).toEqual([
      {
        severity: "warning",
        message: "/stores at desk: a request answered 403",
        at: `${ORIGIN}/api/x`,
        rule: null
      },
      {
        severity: "warning",
        message:
          "/stores at desk: not sent: POST /customer-apps/acme/store/fn/save (the checker reads, and sends no write)",
        at: null,
        rule: "write-refused"
      }
    ]);
  });

  it("is an error, not a pass, when screens were never opened; a failure seen still fails", () => {
    const short = pass("staging", ["", "stores", "plans"]);
    // The time ran out before `plans` at phone width, and before `stores` at either.
    short.visits = short.visits.filter(
      (v) => v.path === "" || (v.path === "plans" && v.viewport.name === "desk")
    );
    short.unvisited = 3;
    const outcome = scoreSmoke(staging, short);
    expect(outcome.status).toBe("error");
    expect(outcome.summary).toBe("1 of 3 screens opened before the time was up");
    expect(outcome.detail.screens_unvisited).toBe(2);
    expect((outcome.detail.screens as { path: string }[]).map((s) => s.path)).toEqual([
      "",
      "plans"
    ]);

    short.visits[0] = {
      ...(short.visits[0] as ScreenVisit),
      observed: seen("staging", { readyMs: null })
    };
    expect(scoreSmoke(staging, short).status).toBe("failed");
  });

  it("puts errors before warnings, so the cut at fifty findings keeps them", () => {
    const noisy = {
      failedRequests: Array.from({ length: 20 }, (_, i) => ({
        url: `${ORIGIN}/api/${i}`,
        status: 500,
        error: null
      }))
    };
    const paths = ["a", "b", "c", "d"];
    const loads: Record<string, Partial<Observed>> = {};
    for (const path of paths) loads[`${path}@desk`] = noisy;
    loads["d@phone"] = { readyMs: null };
    const outcome = scoreSmoke(staging, pass("staging", paths, loads));
    expect(outcome.detail.findings).toHaveLength(50);
    expect(outcome.detail.findings[0]).toMatchObject({ severity: "error" });
    expect(outcome.detail.findings_omitted).toBe(31);
  });

  it("cuts the lists under each load until the detail fits what the platform stores", () => {
    const long = "x".repeat(390);
    const heavy: Partial<Observed> = {
      pageErrors: Array.from({ length: 20 }, (_, i) => `Error ${i}: ${long}`),
      failedRequests: Array.from({ length: 20 }, (_, i) => ({
        url: `${ORIGIN}/api/${i}/${long}`,
        status: 500,
        error: null
      })),
      refused: Array.from({ length: 20 }, (_, i) => `POST /${i}/${long}`)
    };
    const paths = Array.from({ length: 30 }, (_, i) => `s${i}`);
    const loads: Record<string, Partial<Observed>> = {};
    for (const path of paths) for (const v of ["desk", "phone"]) loads[`${path}@${v}`] = heavy;
    const outcome = scoreSmoke(staging, pass("staging", paths, loads));
    const bytes = Buffer.byteLength(JSON.stringify(outcome.detail), "utf8");
    expect(bytes).toBeLessThanOrEqual(256 * 1024);
    // Every screen is still listed; what was cut is the lists under each, and the detail says to what.
    expect(outcome.detail.screens).toHaveLength(30);
    expect(outcome.detail.lists_capped_at).toBe(2);
    expect(outcome.summary).toBe("30 of 30 screens have errors");
  });
});

describe("scoreProductionSmoke", () => {
  const production = target("production");
  const base = (screens: Baseline["screens"]): Baseline => ({
    at: "2026-11-22T10:00:00Z",
    screens
  });
  const row = (path: string, viewportName: string, ready: boolean, errors: string[] = []) => ({
    path,
    viewport: viewportName,
    ready,
    errors
  });
  const OLD = "TypeError: legacy is undefined";

  it("passes when nothing is worse than before, in the contract's shape", () => {
    const outcome = scoreProductionSmoke(
      production,
      pass("production", ["stores"], { "stores@desk": { pageErrors: [OLD] } }),
      base([row("stores", "desk", true, [OLD]), row("stores", "phone", true)])
    );
    expect(outcome).toEqual({
      status: "passed",
      summary: "1 screen, no new errors",
      detail: {
        findings: [],
        baseline_at: "2026-11-22T10:00:00Z",
        discovered: true,
        screens_omitted: 0,
        screens: [
          {
            path: "stores",
            viewports: [
              viewport("desk", { environment: "production", page_errors: [OLD] }),
              viewport("phone", { environment: "production" })
            ]
          }
        ],
        new_errors: []
      }
    });
  });

  it("fails on an error the baseline did not have for that screen and viewport", () => {
    const fresh = "ReferenceError: plans is not defined";
    const outcome = scoreProductionSmoke(
      production,
      pass("production", ["", "stores"], {
        "stores@desk": { pageErrors: [OLD, fresh] },
        // The same old error, where the baseline had it at the other viewport only.
        "stores@phone": { pageErrors: [OLD] }
      }),
      base([
        row("", "desk", true),
        row("", "phone", true),
        row("stores", "desk", true, [OLD]),
        row("stores", "phone", true)
      ])
    );
    expect(outcome.status).toBe("failed");
    expect(outcome.summary).toBe("2 new errors on 1 of 2 screens");
    expect(outcome.detail.new_errors).toEqual([
      { path: "stores", viewport: "desk", error: fresh },
      { path: "stores", viewport: "phone", error: OLD }
    ]);
    expect(outcome.detail.findings).toEqual([
      { severity: "error", message: `/stores at desk: ${fresh}`, at: null, rule: null },
      { severity: "error", message: `/stores at phone: ${OLD}`, at: null, rule: null }
    ]);
  });

  it("fails a screen that was ready before and is not now; one that was not ready before is not news", () => {
    const outcome = scoreProductionSmoke(
      production,
      pass("production", ["stores", "plans"], {
        "stores@desk": { readyMs: null },
        "plans@desk": { readyMs: null }
      }),
      base([
        row("stores", "desk", true),
        row("stores", "phone", true),
        row("plans", "desk", false),
        row("plans", "phone", true)
      ])
    );
    expect(outcome.status).toBe("failed");
    expect(outcome.summary).toBe("1 new error on 1 of 2 screens");
    expect(outcome.detail.new_errors).toEqual([
      {
        path: "stores",
        viewport: "desk",
        error:
          "the app never sent oxy-app-ready within 30s — a blank page, a crash on boot, or a sign-in page"
      }
    ]);
  });

  it("calls nothing new with no baseline: it lists production's errors as warnings and skips", () => {
    const loads = { "stores@desk": { pageErrors: [OLD] }, "new@phone": { readyMs: null } };
    // No baseline at all, and one that saw no screen, read the same.
    for (const none of [undefined, base([])]) {
      const without = scoreProductionSmoke(
        production,
        pass("production", ["stores", "new"], loads),
        none
      );
      expect(without.status).toBe("skipped");
      expect(without.summary).toBe(
        "2 errors on 2 of 2 screens, and no baseline from before the move to tell new ones apart"
      );
      expect(without.detail.baseline_at).toBeNull();
      expect(without.detail.new_errors).toEqual([]);
      const said = without.detail.findings as { severity: string; message: string }[];
      expect(said.map((f) => f.severity)).toEqual(["warning", "warning"]);
      expect(said[0]?.message).toBe(`/stores at desk: ${OLD} (no baseline: not called new)`);
    }
    // Nothing wrong anywhere passes, baseline or not.
    const clean = scoreProductionSmoke(production, pass("production", ["stores"], {}), undefined);
    expect(clean.status).toBe("passed");
  });

  it("calls nothing new on a screen and viewport the baseline does not list: its errors are warnings", () => {
    // The baseline's pass ran out of time before `plans`, and never opened `stores` at phone
    // width. What those show now was not caused by the move, for all anyone can tell.
    const outcome = scoreProductionSmoke(
      production,
      pass("production", ["", "stores", "plans"], {
        "stores@phone": { pageErrors: [OLD] },
        "plans@desk": { pageErrors: [OLD, "RangeError: too many plans"] },
        "plans@phone": { readyMs: null }
      }),
      base([row("", "desk", true), row("", "phone", true), row("stores", "desk", true)])
    );
    // Never `failed` because of them: skipped, as with no baseline at all.
    expect(outcome.status).toBe("skipped");
    expect(outcome.summary).toBe(
      "4 errors on 2 of 3 screens the baseline from before the move cannot tell apart: not called new"
    );
    expect(outcome.detail.new_errors).toEqual([]);
    expect(outcome.detail.baseline_at).toBe("2026-11-22T10:00:00Z");
    const said = outcome.detail.findings as { severity: string; message: string }[];
    expect(said.map((f) => f.severity)).toEqual(["warning", "warning", "warning", "warning"]);
    expect(said.map((f) => f.message)).toEqual([
      `/stores at phone: ${OLD} (not in the baseline: not called new)`,
      `/plans at desk: ${OLD} (not in the baseline: not called new)`,
      "/plans at desk: RangeError: too many plans (not in the baseline: not called new)",
      "/plans at phone: the app never sent oxy-app-ready within 30s — a blank page, a crash on boot, or a sign-in page (not in the baseline: not called new)"
    ]);
  });

  it("still fails on what is new where the baseline saw the screen, beside one it did not", () => {
    const fresh = "ReferenceError: plans is not defined";
    const outcome = scoreProductionSmoke(
      production,
      pass("production", ["stores", "plans"], {
        "stores@desk": { pageErrors: [fresh] },
        "plans@desk": { pageErrors: [OLD] }
      }),
      base([row("stores", "desk", true), row("stores", "phone", true)])
    );
    expect(outcome.status).toBe("failed");
    expect(outcome.summary).toBe("1 new error on 1 of 2 screens");
    expect(outcome.detail.new_errors).toEqual([{ path: "stores", viewport: "desk", error: fresh }]);
    expect(outcome.detail.findings).toMatchObject([
      { severity: "error", message: `/stores at desk: ${fresh}` },
      {
        severity: "warning",
        message: `/plans at desk: ${OLD} (not in the baseline: not called new)`
      }
    ]);
  });

  it("passes when the screens the baseline does not list show nothing wrong", () => {
    const outcome = scoreProductionSmoke(
      production,
      pass("production", ["stores", "plans"], { "stores@desk": { pageErrors: [OLD] } }),
      base([row("stores", "desk", true, [OLD]), row("stores", "phone", true)])
    );
    expect(outcome.status).toBe("passed");
    expect(outcome.summary).toBe("2 screens, no new errors");
    expect(outcome.detail.findings).toEqual([]);
  });

  it("calls no error new against a list that is full: it may have been cut", () => {
    // The screen already threw twelve errors before the move. A baseline keeps ten.
    const twelve = Array.from({ length: 12 }, (_, i) => `Error: old ${i + 1}`);
    const outcome = scoreProductionSmoke(
      production,
      pass("production", ["stores"], { "stores@desk": { pageErrors: twelve } }),
      base([row("stores", "desk", true, twelve.slice(0, 10)), row("stores", "phone", true)])
    );
    expect(outcome.status).toBe("skipped");
    expect(outcome.summary).toBe(
      "2 errors on 1 of 1 screen the baseline from before the move cannot tell apart: not called new"
    );
    expect(outcome.detail.new_errors).toEqual([]);
    expect(outcome.detail.findings).toMatchObject([
      {
        severity: "warning",
        message:
          "/stores at desk: Error: old 11 (the baseline's list of errors is full: not called new)"
      },
      {
        severity: "warning",
        message:
          "/stores at desk: Error: old 12 (the baseline's list of errors is full: not called new)"
      }
    ]);
  });

  it("still calls new, against a full list, what a cut could not have hidden", () => {
    const ten = Array.from({ length: 10 }, (_, i) => `Error: old ${i + 1}`);
    const full = base([row("stores", "desk", true, ten), row("stores", "phone", true, ten)]);
    const outcome = scoreProductionSmoke(
      production,
      pass("production", ["stores"], {
        // Ready before, and not now: no list says otherwise.
        "stores@desk": { pageErrors: ten, readyMs: null },
        // A failed document is the first error of a list, which a cut never drops.
        "stores@phone": { navigationError: "the page answered 502", readyMs: null }
      }),
      full
    );
    expect(outcome.status).toBe("failed");
    expect(outcome.detail.new_errors).toEqual([
      {
        path: "stores",
        viewport: "desk",
        error:
          "the app never sent oxy-app-ready within 30s — a blank page, a crash on boot, or a sign-in page"
      },
      { path: "stores", viewport: "phone", error: "the page answered 502" }
    ]);
    // One short of full, and the list is whole: an error it does not hold is new.
    const nine = ten.slice(0, 9);
    const whole = scoreProductionSmoke(
      production,
      pass("production", ["stores"], { "stores@desk": { pageErrors: ten } }),
      base([row("stores", "desk", true, nine), row("stores", "phone", true)])
    );
    expect(whole.status).toBe("failed");
    expect(whole.detail.new_errors).toEqual([
      { path: "stores", viewport: "desk", error: "Error: old 10" }
    ]);
  });

  it("fails a screen whose document now fails, and one served another build", () => {
    const outcome = scoreProductionSmoke(
      production,
      pass("production", ["stores"], {
        "stores@desk": { navigationError: "the page answered 502", readyMs: null },
        "stores@phone": { app: { environment: "production", buildId: "b0" } }
      }),
      base([row("stores", "desk", true), row("stores", "phone", true)])
    );
    expect(outcome.detail.new_errors).toEqual([
      { path: "stores", viewport: "desk", error: "the page answered 502" },
      {
        path: "stores",
        viewport: "phone",
        error: "the page was served build b0, but production serves b1"
      }
    ]);
  });

  it("does not fail on a failed request or a refused write, before or after", () => {
    const outcome = scoreProductionSmoke(
      production,
      pass("production", ["stores"], {
        "stores@desk": { refused: ["POST /customer-apps/acme/store/fn/save"] }
      }),
      undefined
    );
    expect(outcome.status).toBe("passed");
    expect(outcome.detail.findings).toMatchObject([{ severity: "warning", rule: "write-refused" }]);
  });
});
