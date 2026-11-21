// @vitest-environment jsdom
//
// 2.31.1: the y-axis never prints the same label twice. Its three gridlines sit at the scale's
// low end, middle and high end, labelled in whole percents (a rating: one decimal). A scale under
// two of those steps tall printed one label twice, or three times ("25%", "25%"). Now the labels
// take one more decimal (the readout's own) where two would match, and a scale too narrow for even
// that is widened around its middle, never below zero for data that is not. Everywhere the labels
// already differed, the chart is drawn exactly as before.

import { afterEach, describe, expect, it } from "vitest";
import { $$, render } from "../ui.harness";
import { type ValueUnit, valueAxis } from "./axis";
import { TrendChart } from "./TrendChart";

afterEach(() => {
  document.body.innerHTML = "";
});

const yLabels = () => $$("[data-axis='value']").map((t) => t.textContent);
/** Where the plot starts (its gridlines' x1) and where the y labels end. */
const leftEdge = () => ({
  plot: $$("line").map((l) => l.getAttribute("x1"))[0],
  labelsEnd: $$("[data-axis='value']").map((t) => t.getAttribute("x"))[0]
});

/** The chart's scale and labels as 2.31.0 drew them: the oracle for "unchanged". */
function before(values: number[], unit: ValueUnit) {
  const lo = Math.min(...values);
  const hi = Math.max(...values);
  const spread = unit === "stars" ? 0.1 : 1;
  const domain = hi > lo ? [lo, hi] : [lo - spread, hi + spread];
  const ticks = [domain[0], (domain[0] + domain[1]) / 2, domain[1]];
  return {
    domain,
    labels: ticks.map((t) => (unit === "stars" ? t.toFixed(1) : `${Math.round(t)}%`))
  };
}

/** Low ends and spans, from flat through a hair over flat to a wide scale, for each unit. */
const SWEEP: Record<ValueUnit, { lows: number[]; spans: number[] }> = {
  percent: {
    lows: Array.from({ length: 401 }, (_, i) => i * 0.25),
    spans: [0, 1e-9, 0.001, 0.01, 0.05, 0.1, 0.19, 0.2, 0.3, 0.39, 0.4, 0.45, 0.6, 0.7, 0.9].concat(
      [0.99, 1, 1.01, 1.2, 1.5, 1.99, 2, 2.01, 2.5, 3, 4.9, 7, 10, 25, 37.3, 60]
    )
  },
  stars: {
    lows: Array.from({ length: 401 }, (_, i) => 1 + i * 0.01),
    spans: [0, 1e-9, 0.0001, 0.001, 0.005, 0.01, 0.019, 0.02, 0.03, 0.039, 0.04, 0.05, 0.07].concat(
      [0.09, 0.1, 0.101, 0.15, 0.199, 0.2, 0.25, 0.4, 0.7, 1, 1.5, 2.5, 4]
    )
  }
};

describe("valueAxis", () => {
  for (const unit of ["percent", "stars"] as const) {
    it(`never prints the same ${unit} label twice, on a sweep of scales`, () => {
      let swept = 0;
      let kept = 0;
      for (const lo of SWEEP[unit].lows) {
        for (const span of SWEEP[unit].spans) {
          // Two series' worth of values: the extremes and one between them.
          const values = [lo + span, lo, lo + span / 3];
          const axis = valueAxis(values, unit);
          const at = `${unit} ${lo} + ${span}`;
          expect(new Set(axis.labels).size, `${at}: ${axis.labels.join(", ")}`).toBe(3);
          // Every value is on the scale, and the gridlines are its ends and middle.
          expect(axis.domain[0], at).toBeLessThanOrEqual(lo);
          expect(axis.domain[1], at).toBeGreaterThanOrEqual(lo + span);
          // Data that is not below zero never gets a scale that is, unless it is flat (as before).
          if (span > 0) expect(axis.domain[0], at).toBeGreaterThanOrEqual(0);
          // Wherever 2.31.0's labels already differed, nothing about the chart changes.
          const was = before(values, unit);
          if (new Set(was.labels).size === 3) {
            expect(axis.domain, at).toEqual(was.domain);
            expect(axis.labels, at).toEqual(was.labels);
            kept += 1;
          }
          swept += 1;
        }
      }
      // The sweep held both kinds of scale: ones it left alone and ones it relabelled.
      expect(kept).toBeGreaterThan(swept / 3);
      expect(kept).toBeLessThan(swept);
    });
  }

  it("widens a scale too narrow to label around its middle, from zero for data that is not below it", () => {
    const narrow = valueAxis([25, 25.1, 25.05], "percent");
    expect(narrow.domain.map((v) => Number(v.toFixed(6)))).toEqual([24.85, 25.25]);
    expect(narrow.labels).toEqual(["24.9%", "25.1%", "25.3%"]);
    const nearZero = valueAxis([0, 0.1], "percent");
    expect(nearZero.domain).toEqual([0, 0.4]);
    expect(nearZero.labels).toEqual(["0.0%", "0.2%", "0.4%"]);
    // A flat series keeps its 2.31.0 scale: its value in the middle.
    expect(valueAxis([12, 12], "percent").labels).toEqual(["11%", "12%", "13%"]);
  });
});

describe("TrendChart y labels", () => {
  it("never prints one label twice: a 1.2-point scale reads in tenths", async () => {
    // The reference app's phone store page: "24%", "25%", "25%" before 2.31.1.
    const points = [
      { period: "2026-01", observedPct: 24.2, peerPct: 24.9 },
      { period: "2026-02", observedPct: 25.4, peerPct: 25.1 }
    ];
    await render(<TrendChart points={points} storeLabel='North' />);
    expect(yLabels()).toEqual(["24.2%", "24.8%", "25.4%"]);
    // A tenth's label is wider than a whole percent's: the plot starts 12 further in for it.
    expect(leftEdge()).toEqual({ plot: "52", labelsEnd: "42" });
  });

  it("a rating whose scale is under two tenths reads in hundredths", async () => {
    const points = [
      { period: "2026-01", observedPct: 4.41, peerPct: 4.44 },
      { period: "2026-02", observedPct: 4.47, peerPct: 4.45 }
    ];
    await render(<TrendChart points={points} storeLabel='North' unit='stars' />);
    expect(yLabels()).toEqual(["4.41", "4.44", "4.47"]);
    // "4.47" is no wider than a whole percent: the reference's room.
    expect(leftEdge()).toEqual({ plot: "40", labelsEnd: "30" });
  });
});
