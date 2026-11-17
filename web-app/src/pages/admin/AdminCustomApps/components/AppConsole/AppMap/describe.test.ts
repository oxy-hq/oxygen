import { describe, expect, it } from "vitest";
import type { MapEdge, MapFreshness, MapTarget } from "./appMapModel";
import { freshnessText, runsLabel, targetSummary, touchText } from "./describe";

const ago = (iso: string) => `<${iso.slice(11, 16)}>`;

const freshness = (over: Partial<MapFreshness> = {}): MapFreshness => ({
  perFunction: 5,
  runs: 7,
  oldest: "2026-11-15T08:00:00Z",
  newest: "2026-11-17T10:00:00Z",
  retentionDays: 3,
  quiet: [],
  ...over
});

describe("freshnessText", () => {
  it("names the bound, the run count and the span the right side covers", () => {
    expect(freshnessText(freshness(), ago)).toEqual([
      "Touches come from each function's last 5 recorded runs: 7 runs, from <08:00> to <10:00>. Traces are kept 3 days."
    ]);
  });

  it("names the functions it has nothing on, a few at a time", () => {
    const text = freshnessText(freshness({ quiet: ["a", "b", "c", "d", "e"] }), ago);
    expect(text[1]).toBe("No recorded run for a, b, c and 2 more.");
  });

  it("says the right side is the manifest's alone when nothing ran in the window", () => {
    const text = freshnessText(
      freshness({ runs: 0, oldest: null, newest: null, quiet: ["a"], retentionDays: 1 }),
      ago
    );
    expect(text).toEqual([
      "No recorded run in the last 1 day, so only what the manifest declares is drawn on the right."
    ]);
  });
});

describe("runsLabel", () => {
  const fn = { id: "fn:a", name: "a" };
  it("tells unread, none and some apart", () => {
    expect(runsLabel({ ...fn, runs: null, failedRuns: 0 })).toBe("runs not read yet");
    expect(runsLabel({ ...fn, runs: 0, failedRuns: 0 })).toBe("no recorded run");
    expect(runsLabel({ ...fn, runs: 5, failedRuns: 2 })).toBe("5 recorded runs, 2 failed");
    expect(runsLabel({ ...fn, runs: 1, failedRuns: 0 }, true)).toBe("1 run");
    // On the diagram's node, nothing until there is a run to count.
    expect(runsLabel({ ...fn, runs: 0, failedRuns: 0 }, true)).toBe("");
    expect(runsLabel({ ...fn, runs: null, failedRuns: 0 }, true)).toBe("");
  });
});

describe("touchText and targetSummary", () => {
  const edge: MapEdge = {
    id: "fn:a->target:fetch:x",
    from: "fn:a",
    to: "target:fetch:x",
    kind: "touch",
    families: ["fetch"],
    calls: 4,
    failedCalls: 1,
    declared: false,
    observed: true
  };
  const target: MapTarget = {
    id: "target:fetch:x",
    group: "fetch",
    label: "x.example.com",
    details: ["GET", "POST"],
    calls: 4,
    failedCalls: 1,
    declared: false,
    observed: true
  };

  it("counts calls and failures", () => {
    expect(touchText(edge)).toBe("fetch, 4 calls, 1 failed");
    expect(targetSummary(target)).toBe(
      "Outbound hosts: x.example.com (GET, POST). 4 calls in recent runs, 1 failed."
    );
  });

  it("says a declared destination was not written to, rather than showing zero calls", () => {
    const declared = { ...edge, families: [], calls: 0, failedCalls: 0, observed: false };
    expect(touchText(declared)).toBe("declared destination, not written to in recent runs");
    expect(
      targetSummary({
        ...target,
        group: "warehouse",
        label: "analytics",
        details: [],
        observed: false
      })
    ).toBe("Warehouse writes: analytics. Declared in the manifest; no recorded run wrote to it.");
  });
});
