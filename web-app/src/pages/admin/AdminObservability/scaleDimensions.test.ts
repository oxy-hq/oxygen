import { describe, expect, it } from "vitest";
import type { FleetMedians, FleetRow } from "@/services/api/adminObservability";
import { cellOf, indexOf, maxIndex, SCALE_DIMENSIONS, shownDimensions } from "./scaleDimensions";

const row = (over: Partial<FleetRow>): FleetRow => ({
  org_id: "o",
  org_name: "Org",
  org_slug: "org",
  cost_usd: 0,
  run_count: 0,
  unpriced_runs: 0,
  input_tokens: 0,
  output_tokens: 0,
  cost_per_run_usd: null,
  monthly_run_rate_usd: null,
  unattended_run_share: null,
  cache_hit_share: null,
  requests: null,
  sessions: null,
  people: null,
  apps: null,
  p95_ms: null,
  failure_rate: null,
  function_compute_gbs: null,
  egress_gb: null,
  storage_gib_month: null,
  ...over
});

const medians = (over: Partial<FleetMedians> = {}): FleetMedians => ({
  cost_usd: null,
  run_count: null,
  requests: null,
  function_compute_gbs: null,
  egress_gb: null,
  storage_gib_month: null,
  ...over
});

describe("indexOf", () => {
  /**
   * Everything is infinitely more than nothing. Printing `∞×` or clamping to
   * some large number both assert more than the data supports, so a zero
   * median yields no index and the view says there is no typical account.
   */
  it("has no index against a zero or absent median", () => {
    expect(indexOf(5, 0)).toBeNull();
    expect(indexOf(5, null)).toBeNull();
    expect(indexOf(null, 10)).toBeNull();
  });

  it("is the multiple of the median", () => {
    expect(indexOf(1200, 100)).toBe(12);
    expect(indexOf(50, 100)).toBe(0.5);
  });

  /** Using none of something is a real, legible zero — not a missing index. */
  it("indexes a measured zero at zero", () => {
    expect(indexOf(0, 100)).toBe(0);
  });

  /** A NaN from a future division must not draw a bar of NaN width. */
  it("refuses non-finite inputs", () => {
    expect(indexOf(Number.NaN, 10)).toBeNull();
    expect(indexOf(10, Number.POSITIVE_INFINITY)).toBeNull();
  });
});

describe("shownDimensions", () => {
  /**
   * A deployment with no observability store can answer spend and runs and
   * nothing else. Three columns of dashes say less than their absence, and the
   * caller names what it dropped — the same rule the fleet table applies.
   */
  it("drops a dimension no org can answer", () => {
    const { shown, hidden } = shownDimensions({
      rows: [row({ cost_usd: 10, run_count: 4 })]
    });
    expect(shown.map((d) => d.id)).toEqual(["cost_usd", "run_count"]);
    expect(hidden.map((d) => d.id)).toContain("requests");
    expect(hidden.map((d) => d.id)).toContain("storage_gib_month");
  });

  /** One org answering is enough to keep the axis: that org's figure is real. */
  it("keeps a dimension one org can answer", () => {
    const { shown } = shownDimensions({
      rows: [row({}), row({ storage_gib_month: 4 })]
    });
    expect(shown.map((d) => d.id)).toContain("storage_gib_month");
  });

  /** An empty fleet shows no axes rather than six empty ones. */
  it("shows nothing for an empty fleet", () => {
    expect(shownDimensions({ rows: [] }).shown).toEqual([]);
    expect(shownDimensions({ rows: [] }).hidden).toHaveLength(SCALE_DIMENSIONS.length);
  });
});

describe("cellOf", () => {
  /**
   * The shape the whole view exists to show: an org far above typical on one
   * axis and ordinary on another. A column of absolute figures cannot say
   * this — the biggest tenant is simply biggest everywhere.
   */
  it("reads an org's shape across axes, not just its size", () => {
    const r = row({ cost_usd: 1200, storage_gib_month: 12 });
    const m = medians({ cost_usd: 100, storage_gib_month: 10 });
    expect(cellOf(r, m, "cost_usd").index).toBe(12);
    expect(cellOf(r, m, "storage_gib_month").index).toBe(1.2);
  });

  /** An unmeasured axis keeps its null through to the cell, never a zero. */
  it("carries an unmeasured axis through as null", () => {
    const c = cellOf(row({}), medians({ requests: 500 }), "requests");
    expect(c.value).toBeNull();
    expect(c.index).toBeNull();
  });
});

describe("maxIndex", () => {
  /**
   * One baseline across the whole grid. Scaling each row to its own maximum
   * would make every org's largest axis look identical, erasing the very
   * difference in scale the view is for.
   */
  it("is the widest index anywhere in the grid", () => {
    const rows = [row({ cost_usd: 1200, run_count: 10 }), row({ cost_usd: 100, run_count: 400 })];
    const m = medians({ cost_usd: 100, run_count: 10 });
    // cost: 12x and 1x; runs: 1x and 40x. The widest anywhere is 40.
    expect(maxIndex(rows, m, SCALE_DIMENSIONS)).toBe(40);
  });

  /**
   * Floored at 1. A fleet where nobody exceeds the median would otherwise
   * scale every bar against a sub-1 maximum and draw them all full width.
   */
  it("never falls below one", () => {
    const rows = [row({ cost_usd: 10 })];
    expect(maxIndex(rows, medians({ cost_usd: 100 }), SCALE_DIMENSIONS)).toBe(1);
    expect(maxIndex([], medians(), SCALE_DIMENSIONS)).toBe(1);
  });
});
