import { describe, expect, it } from "vitest";
import type { AppInvocation, AppInvocationList } from "@/types/apps";
import {
  comparePeriods,
  formatChange,
  formatLatency,
  latencyDistribution,
  modeOptions,
  percentile,
  periodCount,
  windowRange
} from "./invocationStats";

const row = (over: Partial<AppInvocation> = {}): AppInvocation => ({
  id: "inv",
  function_name: "sync",
  mode: "route",
  environment: "production",
  build_id: "b-1",
  build_uuid: "00000000-0000-0000-0000-000000000001",
  status: "success",
  failed: false,
  result_status: null,
  duration_ms: 100,
  error: null,
  error_kind: null,
  created_at: "2026-11-17T05:00:00Z",
  has_result: false,
  ...over
});

describe("percentile", () => {
  it("is the nearest rank: always a value that occurred", () => {
    const ten = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    expect(percentile(ten, 50)).toBe(5);
    expect(percentile(ten, 95)).toBe(10);
    expect(percentile(ten, 90)).toBe(9);
    expect(percentile(ten, 0)).toBe(1);
    expect(percentile(ten, 100)).toBe(10);
  });

  it("does not depend on the order it is handed", () => {
    expect(percentile([900, 10, 50, 20], 50)).toBe(20);
  });

  it("is null for nothing, and the value itself for one", () => {
    expect(percentile([], 95)).toBeNull();
    expect(percentile([42], 95)).toBe(42);
  });

  it("puts p95 of twenty on the nineteenth, so one outlier is past it", () => {
    const durations = [...Array.from({ length: 19 }, () => 100), 30_000];
    expect(percentile(durations, 95)).toBe(100);
    expect(percentile([...durations, 30_000], 95)).toBe(30_000);
  });
});

describe("latencyDistribution", () => {
  it("counts every measured invocation once, and names the unmeasured", () => {
    const rows = [
      row({ duration_ms: 80 }),
      row({ duration_ms: 120 }),
      row({ duration_ms: 30_000 }),
      row({ duration_ms: null, status: "running" })
    ];
    const d = latencyDistribution(rows, 12);
    expect(d.buckets).toHaveLength(12);
    expect(d.buckets.reduce((n, b) => n + b.count, 0)).toBe(3);
    expect(d.measured).toBe(3);
    expect(d.unmeasured).toBe(1);
    expect(d.lo).toBe(80);
    expect(d.hi).toBe(30_000);
  });

  it("spreads a long tail on a log axis instead of piling everything in bucket one", () => {
    const rows = [10, 150, 1_500, 10_000].map((ms) => row({ duration_ms: ms }));
    const d = latencyDistribution(rows, 3);
    // Three decades, three buckets: one call in each of the first two, and the
    // two slowest in the last (the top edge is inclusive). On a linear axis all
    // but the slowest would share the first bucket.
    expect(d.buckets.map((b) => b.count)).toEqual([1, 1, 2]);
    expect(d.buckets[0].from).toBeCloseTo(10);
    expect(d.buckets[2].to).toBeCloseTo(10_000);
  });

  it("marks p95 where it falls on the axis", () => {
    const rows = [...Array.from({ length: 19 }, () => 100), 10_000].map((ms) =>
      row({ duration_ms: ms })
    );
    const d = latencyDistribution(rows);
    expect(d.p95).toBe(100);
    expect(d.p95At).toBe(0);
    expect(d.p50).toBe(100);
    const slow = latencyDistribution([100, 10_000].map((ms) => row({ duration_ms: ms })));
    expect(slow.p95).toBe(10_000);
    expect(slow.p95At).toBe(1);
  });

  it("draws nothing from nothing, and keeps a 0 ms call", () => {
    const empty = latencyDistribution([row({ duration_ms: null })]);
    expect(empty.buckets).toEqual([]);
    expect(empty.p95).toBeNull();
    expect(empty.unmeasured).toBe(1);
    const instant = latencyDistribution([row({ duration_ms: 0 }), row({ duration_ms: 5 })], 4);
    expect(instant.buckets.reduce((n, b) => n + b.count, 0)).toBe(2);
    expect(instant.buckets[0].count).toBe(1);
  });
});

describe("periodCount", () => {
  const list = (n: number, failed: number, window?: AppInvocationList["window"]) => ({
    invocations: Array.from({ length: n }, (_, i) => row({ id: `i${i}`, failed: i < failed })),
    window
  });

  it("trusts the server's count of the whole window over the capped page", () => {
    expect(periodCount(list(200, 3, { total: 1412, failed: 37 }), 200)).toEqual({
      total: 1412,
      failed: 37,
      atLeast: false
    });
  });

  it("counts the page when there is no window count — a floor once the page is full", () => {
    expect(periodCount(list(12, 2), 200)).toEqual({ total: 12, failed: 2, atLeast: false });
    expect(periodCount(list(200, 2), 200)).toEqual({ total: 200, failed: 2, atLeast: true });
  });
});

describe("comparePeriods", () => {
  const count = (total: number, failed: number, atLeast = false) => ({ total, failed, atLeast });

  it("states the change as a fraction of the window before", () => {
    const c = comparePeriods(count(1100, 22), count(1000, 10));
    expect(c.totalChange).toBeCloseTo(0.1);
    expect(c.failedChange).toBeCloseTo(1.2);
    expect(c.failureRate.current).toBeCloseTo(0.02);
    expect(c.failureRate.previous).toBeCloseTo(0.01);
  });

  it("has no ratio from nothing to something, and no change from nothing to nothing", () => {
    const c = comparePeriods(count(40, 0), count(0, 0));
    expect(c.totalChange).toBeNull();
    expect(c.failedChange).toBe(0);
    expect(c.failureRate.previous).toBeNull();
  });

  it("refuses to compare a floor", () => {
    const c = comparePeriods(count(200, 4, true), count(150, 1));
    expect(c.totalChange).toBeNull();
    expect(c.failedChange).toBeNull();
    expect(c.failureRate.current).toBeNull();
  });
});

describe("windowRange", () => {
  it("is the window ending now and the equal one before it, touching but not overlapping", () => {
    const anchor = Date.parse("2026-11-17T12:00:00Z");
    expect(windowRange("24h", anchor)).toEqual({
      since: "2026-11-16T12:00:00.000Z",
      until: "2026-11-17T12:00:00.000Z",
      previousSince: "2026-11-15T12:00:00.000Z",
      previousUntil: "2026-11-16T12:00:00.000Z"
    });
    const week = windowRange("7d", anchor);
    expect(week.since).toBe("2026-11-10T12:00:00.000Z");
    expect(week.previousUntil).toBe(week.since);
  });
});

describe("modeOptions", () => {
  it("offers the modes in the data, and keeps the selected one", () => {
    const rows = [row({ mode: "route" }), row({ mode: "schedule" }), row({ mode: "route" })];
    expect(modeOptions(rows)).toEqual(["route", "schedule"]);
    expect(modeOptions([row({ mode: "webhook" })], "manual")).toEqual(["manual", "webhook"]);
  });
});

describe("formatting", () => {
  it("formats latency for an axis and change for a sentence", () => {
    expect(formatLatency(812)).toBe("812ms");
    expect(formatLatency(1_400)).toBe("1.4s");
    expect(formatLatency(42_000)).toBe("42s");
    expect(formatLatency(125_000)).toBe("2m 5s");
    expect(formatChange(0.124)).toBe("+12%");
    expect(formatChange(-0.08)).toBe("−8%");
    expect(formatChange(0.001)).toBe("no change");
  });
});
