import { describe, expect, it } from "vitest";
import queryKeys from "./queryKey";

const BASE = [
  "proj-1",
  "main",
  ["store_days.marketing_spend"],
  "store_days.business_date",
  ["2025-07-20", "2026-07-19"] as [string, string],
  null
] as const;

describe("metricTree.projection key", () => {
  it("separates granularity and horizon", () => {
    const day = queryKeys.metricTree.projection(...BASE, "day", 30);
    const week = queryKeys.metricTree.projection(...BASE, "week", 30);
    const longer = queryKeys.metricTree.projection(...BASE, "day", 60);
    expect(day).not.toEqual(week);
    expect(day).not.toEqual(longer);
  });
});

describe("metricTree.cohort key", () => {
  const PERIOD = ["2026-04-15", "2026-07-13"] as [string, string];

  it("carries every input that changes the answer, branch included", () => {
    // The cohort request carries `?branch=`, and the server treats it as
    // load-bearing: a non-empty branch escalates the request to the ide, which
    // pins a different compiled revision — so two branches can declare
    // different `cohorts:` and answer differently. Asserted as the whole key
    // rather than as an inequality, because these are positional arguments: a
    // pair of keys differs whenever ANY argument moves, which would let a
    // missing branch segment pass an inequality check.
    expect(
      queryKeys.metricTree.cohort(
        "proj-1",
        "feature-x",
        "restaurant",
        "sales.wage_pct",
        "sales.business_date",
        PERIOD,
        null,
        "median"
      )
    ).toEqual([
      "metric-tree",
      "cohort",
      "proj-1",
      "feature-x",
      "restaurant",
      "sales.wage_pct",
      "sales.business_date",
      "2026-04-15",
      "2026-07-13",
      null,
      "median"
    ]);
  });

  it("separates branches", () => {
    const main = queryKeys.metricTree.cohort(
      "proj-1",
      "main",
      "restaurant",
      "sales.wage_pct",
      "sales.business_date",
      PERIOD,
      null,
      "median"
    );
    const feature = queryKeys.metricTree.cohort(
      "proj-1",
      "feature-x",
      "restaurant",
      "sales.wage_pct",
      "sales.business_date",
      PERIOD,
      null,
      "median"
    );
    expect(main).not.toEqual(feature);
  });

  it("separates a named cohort from the measure's default", () => {
    // Omitting `cohort` resolves the measure's `default_cohort:`, which is a
    // different question from naming one.
    const byDefault = queryKeys.metricTree.cohort(
      "proj-1",
      "main",
      "restaurant",
      "sales.wage_pct",
      "sales.business_date",
      PERIOD,
      null,
      "median"
    );
    const named = queryKeys.metricTree.cohort(
      "proj-1",
      "main",
      "restaurant",
      "sales.wage_pct",
      "sales.business_date",
      PERIOD,
      "size_matched",
      "median"
    );
    expect(byDefault).not.toEqual(named);
  });

  it("separates statistics", () => {
    // The request forwards `statistic`, and a median of peers and a best peer
    // are different benchmarks: sharing an entry would serve one for the other
    // for the whole `staleTime`.
    const median = queryKeys.metricTree.cohort(
      "proj-1",
      "main",
      "restaurant",
      "sales.wage_pct",
      "sales.business_date",
      PERIOD,
      null,
      "median"
    );
    const best = queryKeys.metricTree.cohort(
      "proj-1",
      "main",
      "restaurant",
      "sales.wage_pct",
      "sales.business_date",
      PERIOD,
      null,
      "best_peer"
    );
    expect(median).not.toEqual(best);
  });
});
