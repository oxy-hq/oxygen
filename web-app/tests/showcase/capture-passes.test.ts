// Which passes a capture runs, and what it keeps from each. The browser runtime
// is the boundary mocked: each pass answers as scripted and leaves the frame a
// real one would.

import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createMeter } from "../agentic/runner/budget";
import { bespokeRuntime } from "../agentic/runner/runtimes/bespoke";
import type { RuntimeContext } from "../agentic/runner/runtimes/interface";
import type { CaseRunResult } from "../agentic/runner/types";
import { capture } from "./capture";
import { ACTIONS_FILE, type ShowcasePlan } from "./types";

vi.mock("../agentic/runner/runtimes/bespoke", () => ({
  bespokeRuntime: { name: "bespoke", runCase: vi.fn() }
}));

const plan: ShowcasePlan = {
  verdict: "show",
  reason: "r",
  headline: "h",
  start_path: "/local",
  steps: ["Click A", "Click B"],
  expect: "B is open",
  media: "screenshot"
};

const ran = {
  duration_ms: 1,
  step_count: 3,
  tokens: { input: 0, cached_input: 0, cache_creation: 0, output: 0 },
  cache_hits: [],
  step_debug: [],
  judge_usage: {
    calls: 1,
    tokens: { input: 0, cached_input: 0, cache_creation: 0, output: 0 },
    cost_usd: 0
  },
  cost_usd: 0.05
};

type Pass = "drive" | "record" | "replay";
type Scripted = "pass" | "rejected" | "broke";

/** How the runtime was asked to run: the mode `capture` chose for the pass. */
function passOf(ctx: RuntimeContext): Pass {
  if (ctx.cacheMode === "replay") return "replay";
  return ctx.cacheMode === "record" ? "record" : "drive";
}

/** Script the runtime: each pass in order ends as told, having written its frame. */
function script(...ends: Scripted[]): Pass[] {
  const seen: Pass[] = [];
  vi.mocked(bespokeRuntime.runCase).mockImplementation(async (ctx) => {
    const pass = passOf(ctx);
    const end = ends[seen.length];
    seen.push(pass);
    if (end === "broke") {
      return {
        ...ran,
        passed: false,
        error: "no recording for step 'Click A'",
        expect_results: []
      };
    }
    const dir = ctx.capture?.dir ?? "";
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "screenshot.png"), `${pass} frame`);
    if (pass === "record") writeFileSync(ctx.cachePath ?? "", JSON.stringify({ entries: {} }));
    const video = pass === "replay" ? join(dir, "video.webm") : undefined;
    if (video) writeFileSync(video, "film");
    const judged = { kind: "judge" as const, passed: end === "pass", claim: "B is open" };
    const result: CaseRunResult = {
      ...ran,
      passed: end === "pass",
      expect_results: [end === "pass" ? judged : { ...judged, rationale: "A is still open" }],
      capture: { screenshot: join(dir, "screenshot.png"), video, videoStartMs: 0 }
    };
    return result;
  });
  return seen;
}

let dir: string;
const input = (over: Partial<ShowcasePlan> = {}) => ({
  pr: 7,
  plan: { ...plan, ...over },
  startPath: "/local",
  session: { token: "t", user: "{}" },
  dir,
  apiKey: "k",
  meter: createMeter(0.5)
});
const kept = () => readFileSync(join(dir, "media", "screenshot.png"), "utf-8");

beforeEach(() => {
  vi.mocked(bespokeRuntime.runCase).mockReset();
  dir = mkdtempSync(join(tmpdir(), "showcase-capture-"));
});

describe("capture — a screenshot plan", () => {
  // The fault this replaced: a still had to survive a replay to be kept. One
  // replay was judged before a dashboard finished loading; another never ran,
  // because a click that worked had no selector durable enough to record.
  it("is one driven pass: the frame the judge passed is the picture", async () => {
    const seen = script("pass");
    const result = await capture(input());
    expect(seen).toEqual(["drive"]);
    expect(result).toMatchObject({ ok: true, how: "driven and judged", cost_usd: 0.05 });
    expect(kept()).toBe("drive frame");
  });

  it("asks no step for a recording a replay could follow", async () => {
    script("pass");
    await capture(input());
    const ctx = vi.mocked(bespokeRuntime.runCase).mock.calls[0][0];
    expect(ctx.cacheMode).toBeUndefined();
    expect(ctx.flow.settings.cache_actions).toBe(false);
  });

  it("keeps nothing the judge rejected", async () => {
    const seen = script("rejected");
    const result = await capture(input());
    expect(seen).toEqual(["drive"]);
    expect(result).toMatchObject({ ok: false, outcome: "rejected" });
    expect(existsSync(join(dir, "media"))).toBe(false);
  });
});

describe("capture — a video plan", () => {
  it("records, then films the replay", async () => {
    const seen = script("pass", "pass");
    const result = await capture(input({ media: "video" }));
    expect(seen).toEqual(["record", "replay"]);
    expect(result).toMatchObject({ ok: true, how: "driven, then replayed on film" });
    expect(result.ok && result.video).toBe(join(dir, "media", "video.webm"));
    expect(kept()).toBe("replay frame");
  });

  it.each<Scripted>(["rejected", "broke"])(
    "keeps the still when the replay is %s — it costs the video, not the picture",
    async (replayEnds) => {
      const seen = script("pass", replayEnds);
      const result = await capture(input({ media: "video" }));
      expect(seen).toEqual(["record", "replay"]);
      expect(result.ok && result.how).toMatch(/^driven and judged; no video — /);
      expect(result.ok && result.video).toBeUndefined();
      expect(kept()).toBe("record frame");
      expect(existsSync(join(dir, "media", "video.webm"))).toBe(false);
    }
  );
});

describe("capture — a plan with no steps", () => {
  it("is one filmed pass over the start page, and nothing drives it when that fails", async () => {
    const seen = script("rejected");
    const result = await capture(input({ steps: [] }));
    expect(seen).toEqual(["replay"]);
    expect(result).toMatchObject({ ok: false, outcome: "rejected" });
  });

  it("keeps the start page the judge passed", async () => {
    const seen = script("pass");
    const result = await capture(input({ steps: [] }));
    expect(seen).toEqual(["replay"]);
    expect(result).toMatchObject({ ok: true, how: "the start page shows it" });
    expect(kept()).toBe("replay frame");
  });
});

describe("capture — a recording from somewhere else", () => {
  // A release used to replay the recording a preview's artifact carried. A
  // recording is now only ever replayed by the run that made it.
  it("is never replayed: the plan is driven, and the file is gone before anything runs", async () => {
    const planted = join(dir, ACTIONS_FILE);
    writeFileSync(
      planted,
      JSON.stringify({ entries: { k: { actions: [{ tool: "browser_navigate" }] } } })
    );
    let presentWhenDriven = true;
    const seen = script("pass");
    const scripted = vi.mocked(bespokeRuntime.runCase).getMockImplementation();
    vi.mocked(bespokeRuntime.runCase).mockImplementation(async (ctx) => {
      presentWhenDriven = existsSync(planted);
      return scripted ? scripted(ctx) : Promise.reject(new Error("unscripted"));
    });
    const result = await capture(input());
    expect(seen).toEqual(["drive"]);
    expect(presentWhenDriven).toBe(false);
    expect(result).toMatchObject({ ok: true, how: "driven and judged" });
  });
});
