import { describe, expect, it } from "vitest";
import type { DailySeries } from "@/types/experiment";
import { rollingMean, seriesChartOptions, seriesRows, seriesSummary } from "./series";

const colors = { treated: "#t", control: "#c", band: "#b" };

const waves: DailySeries = {
  measure: "store_days.revenue",
  measured_until: null,
  design: "waves",
  treated_units: 2,
  control_units: 1,
  waves: [
    {
      switch_on: "2026-03-05",
      units: 2,
      pre_from: "2026-03-02",
      pre_to: "2026-03-04",
      post_from: "2026-03-06",
      post_to: "2026-03-08"
    }
  ],
  days: [
    { date: "2026-03-02", treated: 3, control: 10, on_units: 0 },
    { date: "2026-03-04", treated: 5, control: 10, on_units: 0 },
    { date: "2026-03-05", treated: 6, control: null, on_units: 2 },
    { date: "2026-03-06", treated: 7.5, control: 10, on_units: 2 }
  ]
};

const staggered: DailySeries = {
  ...waves,
  treated_units: 3,
  waves: [
    {
      ...waves.waves[0],
      units: 1,
      switch_on: "2026-03-03",
      post_from: "2026-03-03",
      post_to: "2026-03-06"
    },
    { ...waves.waves[0], units: 2, pre_from: "2026-03-04", pre_to: "2026-03-05" }
  ],
  days: [{ date: "2026-03-04", treated: 1, control: 1, on_units: 1 }]
};

const switchback: DailySeries = {
  measure: "store_days.revenue",
  measured_until: "2026-04-06",
  design: "switchback",
  fleet_units: 2,
  washout_days: 1,
  periods: [
    { from: "2026-04-01", to: "2026-04-04", on: true },
    { from: "2026-04-04", to: "2026-04-07", on: false }
  ],
  days: [
    { date: "2026-04-01", fleet: 2, on: true, washout: true },
    { date: "2026-04-02", fleet: 3, on: true, washout: false },
    { date: "2026-04-04", fleet: 5, on: false, washout: true }
  ]
};

describe("seriesRows", () => {
  it("gives a waves day its arm means, difference, lever state and window", () => {
    const rows = seriesRows(waves);
    expect(rows.map((r) => r.date)).toEqual([
      "2026-03-02",
      "2026-03-04",
      "2026-03-05",
      "2026-03-06"
    ]);
    expect(rows[0]).toMatchObject({
      treated: 3,
      control: 10,
      difference: -7,
      lever: "Off",
      window: "Pre window"
    });
    // 03-04 is the anticipation day: outside both windows.
    expect(rows[1].window).toBe("");
    expect(rows[2]).toMatchObject({ difference: null, lever: "On", window: "" });
    expect(rows[3]).toMatchObject({ difference: -2.5, window: "Post window" });
  });

  it("names the wave on a staggered design and counts units on", () => {
    const [row] = seriesRows(staggered);
    expect(row.lever).toBe("1 of 3 on");
    expect(row.window).toBe("Wave 1 post, wave 2 pre");
  });

  it("gives a switchback day the fleet mean, the period state and its washout", () => {
    const rows = seriesRows(switchback);
    expect(rows.map((r) => [r.fleet, r.lever, r.window])).toEqual([
      [2, "On", "Washout"],
      [3, "On", ""],
      [5, "Off", "Washout"]
    ]);
  });
});

describe("seriesSummary", () => {
  it("states the arms and the span", () => {
    expect(seriesSummary(waves)).toBe(
      "store_days.revenue per unit-day, 2 treated and 1 control units, 2026-03-02 to 2026-03-06 (4 days)."
    );
  });

  it("states the fleet and the abort cut", () => {
    expect(seriesSummary(switchback)).toBe(
      "store_days.revenue per unit-day, mean over a fleet of 2 units, 2026-04-01 to 2026-04-04 (3 days). Measurement stopped before 2026-04-06."
    );
  });
});

describe("seriesChartOptions", () => {
  it("draws both arms over the dates and shades each post window", () => {
    const o = seriesChartOptions(waves, colors);
    expect(o.xAxis).toMatchObject({ data: waves.days.map((d) => d.date) });
    const series = o.series as { name: string; data: (number | null)[]; markArea?: unknown }[];
    expect(series.map((s) => s.name)).toEqual([
      "Treated (2 units)",
      "Control (1 unit)",
      "Treated, 7-day average",
      "Control, 7-day average"
    ]);
    expect(series[1].data).toEqual([10, 10, null, 10]);
    expect(series[0].markArea).toMatchObject({
      data: [[{ xAxis: "2026-03-06", name: "Post window" }, { xAxis: "2026-03-06" }]]
    });
  });

  it("draws the fleet mean and shades the on periods of a switchback", () => {
    const o = seriesChartOptions(switchback, colors);
    const series = o.series as { name: string; data: number[]; markArea?: unknown }[];
    expect(series.map((s) => s.name)).toEqual(["Fleet mean (2 units)"]);
    expect(series[0].data).toEqual([2, 3, 5]);
    expect(series[0].markArea).toMatchObject({
      data: [[{ xAxis: "2026-04-01", name: "On" }, { xAxis: "2026-04-02" }]]
    });
  });

  it("overlays each arm's 7-day average as a bold line over its faded daily line", () => {
    const o = seriesChartOptions(waves, colors);
    const series = o.series as {
      data: (number | null)[];
      color: string;
      lineStyle?: { width?: number; opacity?: number };
    }[];
    const [treated, control, treatedTrend, controlTrend] = series;
    expect(treatedTrend.color).toBe("#t");
    expect(controlTrend.color).toBe("#c");
    expect(treatedTrend.data).toEqual(
      rollingMean(
        waves.days.map((d) => d.date),
        [3, 5, 6, 7.5]
      )
    );
    expect(controlTrend.data).toEqual([10, 10, 10, 10]);
    expect(treated.lineStyle?.opacity).toBeLessThan(1);
    expect(control.lineStyle?.opacity).toBeLessThan(1);
    expect(treatedTrend.lineStyle?.width).toBeGreaterThan(treated.lineStyle?.width ?? 2);
    // An average is rarely a round number; the tooltip shows it as money, like the table.
    const tooltip = o.tooltip as { valueFormatter: (v: unknown) => string };
    expect(tooltip.valueFormatter(14 / 3)).toBe("$4.67");
  });
});

describe("rollingMean", () => {
  it("averages the calendar days within three either side, skipping gaps and missing values", () => {
    const dates = [
      "2026-03-01",
      "2026-03-02",
      "2026-03-04",
      "2026-03-05",
      "2026-03-08",
      "2026-03-12"
    ];
    expect(rollingMean(dates, [1, 2, null, 4, 8, 16])).toEqual([
      // 03-01 sees 03-01..03-04: 1, 2, (null)
      1.5,
      // 03-02 sees 03-01..03-05: 1, 2, 4
      7 / 3,
      // 03-04 sees 03-01..03-07: 1, 2, 4 — its own value is missing but the trend is not
      7 / 3,
      // 03-05 sees 03-02..03-08: 2, 4, 8
      14 / 3,
      // 03-08 sees 03-05..03-11: 4, 8
      6,
      // 03-12 sees 03-09..03-15: 16
      16
    ]);
  });

  it("leaves a day with nothing in its window empty, never 0", () => {
    expect(rollingMean(["2026-03-01", "2026-03-20"], [null, 5])).toEqual([null, 5]);
  });
});

describe("a week-grain series", () => {
  const weekly: DailySeries = {
    ...waves,
    grain: "week",
    days: [
      { date: "2026-03-02", treated: 1, control: 1, on_units: 0 },
      { date: "2026-03-09", treated: 2, control: 1, on_units: 0 },
      { date: "2026-03-16", treated: 9, control: 1, on_units: 2 }
    ]
  } as DailySeries;

  it("is worded per unit-week and counts weeks", () => {
    expect(seriesSummary(weekly)).toBe(
      "store_days.revenue per unit-week, 2 treated and 1 control units, 2026-03-02 to 2026-03-16 (3 weeks)."
    );
  });

  it("averages three weeks around each point", () => {
    const series = seriesChartOptions(weekly, colors).series as {
      name: string;
      data: (number | null)[];
    }[];
    expect(series[2].name).toBe("Treated, 3-week average");
    expect(series[2].data).toEqual([1.5, 4, 5.5]);
  });
});

describe("seriesRows on a retrospective", () => {
  it("shows the lever as never switched, not On or Off", () => {
    const rows = seriesRows(waves, { retrospective: true });
    expect(rows.map((r) => r.lever)).toEqual(["—", "—", "—", "—"]);
  });
});
