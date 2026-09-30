import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { passFailure, portableRecording, showcaseFlow } from "./capture";
import type { ShowcasePlan } from "./types";

// The literal placeholder the plan writes (built, so it reads as data, not a template).
const RUN = "$" + "{SHOWCASE_RUN}";

describe("the run token", () => {
  const creating = {
    verdict: "show" as const,
    reason: "r",
    headline: "h",
    start_path: "/acme",
    steps: [`Type 'Front counter ${RUN}' into the Name field`],
    expect: `An 'Enroll Front counter ${RUN}' dialog with a QR code`,
    media: "screenshot" as const
  };
  it("keeps the placeholder in the step (the cache key) and expands it for the judge", () => {
    const [testCase] = showcaseFlow(1, creating, "3f9a2c1b").cases;
    expect(testCase.steps[0].act).toContain(RUN);
    expect(testCase.expect).toEqual([
      { judge: "An 'Enroll Front counter 3f9a2c1b' dialog with a QR code" }
    ]);
  });
});

describe("portableRecording", () => {
  const write = (url: string) => {
    const path = join(mkdtempSync(join(tmpdir(), "rec-")), "actions.json");
    writeFileSync(
      path,
      JSON.stringify({ entries: { k: { actions: [{ tool: "browser_navigate", args: { url } }] } } })
    );
    return path;
  };
  it("rewrites an absolute URL to its path, so it replays on another port", () => {
    const path = write("http://127.0.0.1:4173/admin/apps?tab=all");
    expect(portableRecording(path)).toBeUndefined();
    const saved = JSON.parse(readFileSync(path, "utf-8"));
    expect(saved.entries.k.actions[0].args.url).toBe("/admin/apps?tab=all");
  });
  it("keeps the Demo workspace id, which every seed shares", () => {
    expect(
      portableRecording(write("/local/workspaces/70787bb2-e11b-5488-b2c3-02e60d5fc7d3/ide"))
    ).toBeUndefined();
  });
  it("refuses an id one seed minted", () => {
    expect(
      portableRecording(write("/acme/workspaces/0b8c3f2e-1111-4222-8333-944455556666/home"))
    ).toMatch(/minted/);
  });
});

const plan: ShowcasePlan = {
  verdict: "show",
  reason: "r",
  headline: "h",
  start_path: "/local",
  steps: ["Click A", "Click B"],
  expect: "B is open",
  media: "screenshot"
};

const passed = {
  passed: true,
  duration_ms: 1,
  step_count: 3,
  tokens: { input: 0, cached_input: 0, cache_creation: 0, output: 0 },
  cache_hits: [],
  expect_results: [{ kind: "judge" as const, passed: true, claim: "B is open" }],
  step_debug: [],
  judge_usage: {
    calls: 1,
    tokens: { input: 0, cached_input: 0, cache_creation: 0, output: 0 },
    cost_usd: 0
  },
  cost_usd: 0
};

describe("showcaseFlow", () => {
  it("keys its recording on the PR, not on where the checkout lives", () => {
    const flow = showcaseFlow(3375, plan);
    expect(flow.file).toBe("showcase:pr-3375");
    expect(flow.setup).toEqual([]);
  });
  it("runs the plan's steps, settles, and judges its expectation", () => {
    const [testCase] = showcaseFlow(1, plan).cases;
    expect(testCase.steps).toEqual([
      { act: "Click A" },
      { act: "Click B" },
      { wait_for: "network_idle" }
    ]);
    expect(testCase.expect).toEqual([{ judge: "B is open" }]);
  });
});

describe("passFailure", () => {
  it("keeps a run whose steps and judge passed", () => {
    expect(passFailure(passed)).toBeUndefined();
  });
  it("calls a step error a failure", () => {
    expect(
      passFailure({ ...passed, passed: false, error: "no recording for step 'Click A'" })
    ).toEqual({
      outcome: "failed",
      reason: "no recording for step 'Click A'"
    });
  });
  it("calls a judge API error a retryable failure, not a verdict on the screen", () => {
    const judged = [
      {
        kind: "judge" as const,
        passed: false,
        claim: "B is open",
        rationale: "judge API error 529: overloaded"
      }
    ];
    expect(passFailure({ ...passed, passed: false, expect_results: judged })).toEqual({
      outcome: "failed",
      reason: "judge API error 529: overloaded",
      retryable: true
    });
  });
  it("calls a judge rejection a rejection, with its rationale", () => {
    const judged = [
      { kind: "judge" as const, passed: false, claim: "B is open", rationale: "A is still open" }
    ];
    expect(passFailure({ ...passed, passed: false, expect_results: judged })).toEqual({
      outcome: "rejected",
      reason: "the final screen did not show it: A is still open"
    });
  });
});
