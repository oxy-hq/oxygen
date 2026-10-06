import { describe, expect, it } from "vitest";
import type { MonitorCoverage, MonitorEntry } from "@/types/metricAnomalies";
import { coverageFor, filterKey, sensitivityVariant, warmingSummary } from "./monitorRows";

const segment = (over: Partial<MonitorCoverage>): MonitorCoverage => ({
  id: "c",
  workspace_id: "w",
  measure: "sales.net",
  time_dimension: "sales.day",
  granularity: "day",
  dimension_key: "",
  filters: null,
  label: null,
  measured_buckets: 56,
  required_buckets: 56,
  last_scanned_at: "2026-10-06T00:00:00Z",
  ...over
});

const entry = (over: Partial<MonitorEntry>): MonitorEntry => ({
  measure: "sales.net",
  time_dimension: "sales.day",
  granularity: "day",
  lookback_days: 90,
  seasonality: null,
  sensitivity: "medium",
  ...over
});

describe("sensitivityVariant", () => {
  // Sensitivity is a setting. The error colour on `high` made a monitor doing
  // what it was told look like a monitor that was failing.
  it("never badges a setting in the error colour", () => {
    const variants = (["low", "medium", "high"] as const).map(sensitivityVariant);
    expect(variants).toEqual(["outline", "outline", "secondary"]);
    expect(variants).not.toContain("destructive");
  });
});

describe("warmingSummary", () => {
  it("has nothing to say about a monitor that is being scored", () => {
    expect(warmingSummary([segment({})])).toBeNull();
    expect(warmingSummary([])).toBeNull();
  });

  it("names the real numbers for a single segment", () => {
    expect(
      warmingSummary([segment({ granularity: "week", measured_buckets: 9, required_buckets: 26 })])
    ).toEqual({ label: "Warming up", detail: ["9 of 26 weeks"] });
  });

  it("reports a fanned-out monitor as separate facts, with the segment furthest behind", () => {
    const rows = [
      segment({ dimension_key: "s=1", measured_buckets: 50 }),
      segment({ dimension_key: "s=2", measured_buckets: 22 }),
      segment({ dimension_key: "s=3" })
    ];
    expect(warmingSummary(rows)).toEqual({
      label: "Partly warming up",
      detail: ["2 of 3 segments", "furthest 22 of 56"]
    });
    expect(warmingSummary(rows.slice(0, 2))?.label).toBe("Warming up");
  });
});

describe("coverageFor", () => {
  const us = segment({ dimension_key: "sales.region=US" });
  const eu = segment({ dimension_key: "sales.region=EU" });
  const usStore = segment({ dimension_key: "sales.region=US;sales.store=12" });

  it("gives an entry without group_by only its own segment", () => {
    const filters = [{ member: "sales.region", values: ["US"] }];
    expect(filterKey(filters)).toBe("sales.region=US");
    expect(coverageFor(entry({ filters }), [us, eu, usStore])).toEqual([us]);
  });

  it("gives a group_by entry every segment that carries its filters", () => {
    const filters = [{ member: "sales.region", values: ["US"] }];
    expect(coverageFor(entry({ filters, group_by: "sales.store" }), [us, eu, usStore])).toEqual([
      us,
      usStore
    ]);
    expect(coverageFor(entry({ group_by: "sales.store" }), [us, eu])).toEqual([us, eu]);
  });
});
