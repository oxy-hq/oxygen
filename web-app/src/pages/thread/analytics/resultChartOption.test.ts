import { describe, expect, it } from "vitest";
import type { ResultChartData } from "@/services/api/analytics";
import { resultChartOption } from "./resultChartOption";

const chart = (over: Partial<ResultChartData>): ResultChartData => ({
  result_id: "r1",
  kind: "line",
  x: "month",
  y: ["delta"],
  rows: [],
  ...over
});

// biome-ignore lint/suspicious/noExplicitAny: reading ECharts' loose option shape in asserts
const seriesOf = (o: any) => o.series as { name: string; data: unknown[] }[];

describe("resultChartOption", () => {
  // The bug this replaces: a hidden SUM turned signs and decimals into other numbers.
  it("draws negatives, decimals and nulls as given, in the returned order", () => {
    const o = resultChartOption(
      chart({
        y: ["delta", "rate"],
        rows: [
          ["2026-03", -12.5, 0.30000000000000004],
          ["2026-01", null, -0.0001],
          ["2026-02", 7, -9007199254740991]
        ]
      }),
      []
    );
    // biome-ignore lint/suspicious/noExplicitAny: ECharts option shape
    expect((o.xAxis as any).data).toEqual(["2026-03", "2026-01", "2026-02"]);
    expect(seriesOf(o).map((s) => s.data)).toEqual([
      [-12.5, null, 7],
      [0.30000000000000004, -0.0001, -9007199254740991]
    ]);
  });

  it("splits one measure by series, aligned to x, a missing pair a gap", () => {
    const o = resultChartOption(
      chart({
        kind: "bar",
        series: "region",
        y: ["n"],
        rows: [
          ["jan", "east", 1],
          ["jan", "west", -2],
          ["feb", "east", 3]
        ]
      }),
      []
    );
    // biome-ignore lint/suspicious/noExplicitAny: ECharts option shape
    expect((o.xAxis as any).data).toEqual(["jan", "feb"]);
    expect(seriesOf(o)).toMatchObject([
      { name: "east", data: [1, 3] },
      { name: "west", data: [-2, null] }
    ]);
  });

  it("keeps a repeated x on a scatter as separate points", () => {
    const o = resultChartOption(
      chart({
        kind: "scatter",
        x: "price",
        y: ["units"],
        rows: [
          [2.5, 10],
          [2.5, -4]
        ]
      }),
      []
    );
    expect(seriesOf(o)[0].data).toEqual([
      [2.5, 10],
      [2.5, -4]
    ]);
  });

  it("labels pie slices, a null label named rather than dropped", () => {
    const o = resultChartOption(
      chart({
        kind: "pie",
        x: "k",
        y: ["v"],
        rows: [
          ["a", 3],
          [null, 1.25]
        ]
      }),
      []
    );
    expect(seriesOf(o)[0].data).toEqual([
      { name: "a", value: 3 },
      { name: "(null)", value: 1.25 }
    ]);
  });
});
