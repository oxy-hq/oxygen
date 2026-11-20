// @vitest-environment jsdom
//
// The expanded trend. First Watchlist's `TrendChart.test.ts`, ported whole: month labels never
// draw into each other on any span, start month or width. Then the frame as a reader gets it: its
// name from the caption, the paths for given data on one scale, a flat series on a scale its line
// sits on, the empty and one-point sentence, the end labels kept apart, the month read by hover,
// by keyboard and by tap, the readout's arithmetic from props, the subject's tone, the app's marks,
// and only theme tokens painted.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { $, $$, fire, press, render } from "../ui.harness";
import { axisTickLabels, labelledIndices } from "./axis";
import { paints } from "./paint.harness";
import type { TrendPoint } from "./series";
import { TrendChart } from "./TrendChart";

afterEach(() => {
  document.body.innerHTML = "";
});

/**
 * Month labels as Chrome draws them at 12px in the system face, measured over every month of
 * 2024-2027 (Watchlist's measure-labels script): a year-bearing label ("May 2026") at most
 * 56.5px, a bare month ("May") at most 23.4px. The widths the labels have on the glass,
 * independent of the estimates the thinning uses, so the test checks the thinning against the
 * drawing.
 */
const DRAWN_W = { year: 56.5, month: 23.4 };

function months(start: string, n: number): string[] {
  let [y, m] = start.split("-").map(Number);
  const out: string[] = [];
  for (let i = 0; i < n; i++) {
    out.push(`${y}-${String(m).padStart(2, "0")}`);
    m++;
    if (m > 12) {
      m = 1;
      y++;
    }
  }
  return out;
}

/** The smallest gap, in CSS px, between two neighbouring labels' ink, anchored as the chart
 *  anchors them. */
function minGap(periods: string[], plotW: number, unitsPerPx = 1): number {
  const shown = [...labelledIndices(periods, plotW, unitsPerPx)].sort((a, b) => a - b);
  const text = axisTickLabels(periods, shown);
  const n = periods.length;
  const spans = shown.map((i) => {
    const w = (/\d{4}/.test(text.get(i) ?? "") ? DRAWN_W.year : DRAWN_W.month) * unitsPerPx;
    const x = (i * plotW) / (n - 1);
    return i === 0 ? [x, x + w] : i === n - 1 ? [x - w, x] : [x - w / 2, x + w / 2];
  });
  let gap = Number.POSITIVE_INFINITY;
  for (let k = 1; k < spans.length; k++) gap = Math.min(gap, spans[k][0] - spans[k - 1][1]);
  return gap / unitsPerPx;
}

describe("labelledIndices (Watchlist's TrendChart.test.ts)", () => {
  // Plot widths, with the SVG units per CSS px each is drawn at: phone sheets at 390 and 430 and
  // the landscape sheet draw 1:1; the desk's fixed 438-unit plot is drawn at the panel's width
  // (694 in a detail, 660 on a page, 566 at the 640px desk boundary) over 560 units.
  const PLOTS: Record<string, [number, number]> = {
    phone390: [212, 1],
    phone430: [244, 1],
    landscape: [666, 1],
    desk694: [438, 560 / 694],
    desk660: [438, 560 / 660],
    desk566: [438, 560 / 566],
    // The SDK's own plot (the reference's geometry): 432 wide, labels at the xs step in units.
    sdk: [432, 1]
  };

  it("never draws two month labels into each other, on any span or start month", () => {
    for (const [where, [plotW, unitsPerPx]] of Object.entries(PLOTS)) {
      for (const span of [3, 6, 12, 24]) {
        for (let m = 1; m <= 12; m++) {
          const periods = months(`2024-${String(m).padStart(2, "0")}`, span);
          expect(
            minGap(periods, plotW, unitsPerPx),
            `${where} ${span} months from 2024-${m}`
          ).toBeGreaterThanOrEqual(2);
        }
      }
    }
  });

  it("keeps the first and the last month, and a label in between where there is room", () => {
    const periods = months("2024-11", 24);
    const shown = labelledIndices(periods, PLOTS.phone390[0]);
    expect(shown.has(0)).toBe(true);
    expect(shown.has(23)).toBe(true);
    expect(shown.size).toBeGreaterThan(2);
    // "Nov 2024Jun 2025": the second label used to sit 7px into the first.
    expect([...shown].sort((a, b) => a - b)[1]).toBeGreaterThan(7);
  });

  it("puts the year on the first tick and on each new year's only", () => {
    const periods = months("2025-11", 4);
    expect([...axisTickLabels(periods, [0, 1, 2, 3]).values()]).toEqual([
      "Nov 2025",
      "Dec",
      "Jan 2026",
      "Feb"
    ]);
    expect(labelledIndices(["2026-01"], 400)).toEqual(new Set([0]));
  });
});

const TWO: TrendPoint[] = [
  { period: "2026-01", observedPct: 10, peerPct: 20 },
  { period: "2026-02", observedPct: 30, peerPct: 20 }
];
const YEAR: TrendPoint[] = months("2025-07", 12).map((period, i) => ({
  period,
  observedPct: 20 + i,
  peerPct: 18
}));

const svg = () => $("svg") as SVGSVGElement & HTMLElement;
const plot = () => $(".oxy-ui-trend-chart__plot") as HTMLElement;
const d = (series: string) => $(`path[data-series='${series}']`)?.getAttribute("d");
const said = () => plot().getAttribute("aria-valuetext") ?? "";
const now = () => plot().getAttribute("aria-valuenow");
const hover = (i: number) =>
  fire($(`[data-hit='${i}']`), new MouseEvent("mouseover", { bubbles: true }));

describe("TrendChart", () => {
  it("is named by its caption, a default one that says what the axis is", async () => {
    const page = await render(<TrendChart points={TWO} storeLabel='North' />);
    const caption = $("figcaption") as HTMLElement;
    expect(caption.textContent).toBe("Share of sales, month by month");
    // A slider over the months, named by the caption, its value the latest month at rest.
    expect(plot().getAttribute("role")).toBe("slider");
    expect(plot().getAttribute("aria-labelledby")).toBe(caption.id);
    // The drawing is the slider's picture, hidden from assistive tech.
    expect(svg().getAttribute("aria-hidden")).toBe("true");
    expect([plot().getAttribute("aria-valuemin"), plot().getAttribute("aria-valuemax")]).toEqual([
      "0",
      "1"
    ]);
    expect(now()).toBe("1");
    expect(said()).toBe("Feb 2026 — North 30.0%, comparable 20.0%");
    await page.rerender(<TrendChart points={TWO} storeLabel='North' unit='stars' />);
    expect(caption.textContent).toBe("Average rating, month by month");
    await page.rerender(<TrendChart points={TWO} storeLabel='North' caption='Labor' />);
    expect(caption.textContent).toBe("Labor");
  });

  it("draws the given data on the reference's geometry: 560 x 178, a 432 x 140 plot", async () => {
    await render(<TrendChart points={TWO} storeLabel='North' />);
    expect(svg().getAttribute("viewBox")).toBe("0 0 560 178");
    expect(d("observed")).toBe("M0.00,140.00 L432.00,0.00");
    expect(d("peer")).toBe("M0.00,70.00 L432.00,70.00");
    expect($("path[data-series='observed']")?.getAttribute("transform")).toBe("translate(40,14)");
    // Three gridlines across the plot, 14 under the top, the lowest at 154.
    const grid = $$("line").filter((l) => l.getAttribute("stroke") === "var(--border-subtle)");
    expect(
      grid.map((l) => [l.getAttribute("x1"), l.getAttribute("x2"), l.getAttribute("y1")])
    ).toEqual([
      ["40", "472", "154"],
      ["40", "472", "84"],
      ["40", "472", "14"]
    ]);
    const ticks = $$("[data-axis='value']");
    expect(ticks.map((t) => t.textContent)).toEqual(["10%", "20%", "30%"]);
    // Each y label ends 10 short of its gridline, on the line's height plus 3.
    expect(ticks.map((t) => [t.getAttribute("x"), t.getAttribute("y")])).toEqual([
      ["30", "157"],
      ["30", "87"],
      ["30", "17"]
    ]);
  });

  it("sets the months in their own band, 18 under the plot, anchored inward at the ends", async () => {
    await render(<TrendChart points={YEAR.slice(0, 6)} storeLabel='North' />);
    const months = $$("[data-axis='month']");
    expect(months.length).toBeGreaterThanOrEqual(2);
    expect(new Set(months.map((m) => m.getAttribute("y")))).toEqual(new Set(["172"]));
    expect([months[0].getAttribute("x"), months[0].getAttribute("text-anchor")]).toEqual([
      "40",
      "start"
    ]);
    const last = months[months.length - 1];
    expect([last.getAttribute("x"), last.getAttribute("text-anchor")]).toEqual(["472", "end"]);
    // The end labels 8 past the plot; the hit areas the chart's whole height.
    expect($$(".oxy-ui-trend-chart__end").map((t) => t.getAttribute("x"))).toEqual(["480", "480"]);
    expect(new Set($$("[data-hit]").map((r) => r.getAttribute("height")))).toEqual(
      new Set(["178"])
    );
  });

  it("sizes its labels in viewBox units, so they scale with the chart as the reference's", async () => {
    await render(<TrendChart points={TWO} storeLabel='North' />);
    const labels = $$("svg text");
    expect(labels.length).toBe(7);
    for (const t of labels) expect(t.getAttribute("style"), t.textContent ?? "").toBeNull();
    const css = readFileSync(
      join(dirname(fileURLToPath(import.meta.url)), "charts.css"),
      "utf8"
    ).replace(/\/\*[\s\S]*?\*\//g, "");
    for (const cls of ["__tick", "__end"]) {
      const at = css.indexOf(`.oxy-ui-trend-chart${cls} {`);
      expect(at, cls).toBeGreaterThanOrEqual(0);
      expect(css.slice(at, css.indexOf("}", at)), cls).toMatch(
        /font-size: var\(--type-scale-xs-size\);/
      );
    }
  });

  it("gives a flat series a scale its line sits on, its value in the middle", async () => {
    const flat = TWO.map((p) => ({ ...p, observedPct: 12, peerPct: 12 }));
    await render(<TrendChart points={flat} storeLabel='North' />);
    expect(d("observed")).toBe("M0.00,70.00 L432.00,70.00");
    const ticks = $$("[data-axis='value']");
    expect(ticks.map((t) => t.textContent)).toEqual(["11%", "12%", "13%"]);
  });

  it("says there is not enough history for empty data and for one point", async () => {
    for (const points of [[], TWO.slice(0, 1)]) {
      const page = await render(
        <TrendChart points={points} storeLabel='North' emptyText='No trend yet.' />
      );
      expect($("svg")).toBeNull();
      expect($(".oxy-ui-trend-chart__empty")?.textContent).toBe("No trend yet.");
      await page.unmount();
    }
  });

  it("direct-labels both lines and pushes the labels apart where the lines meet", async () => {
    const meet = [TWO[0], { ...TWO[1], observedPct: 20 }];
    await render(<TrendChart points={meet} storeLabel='North' peerLabel='All stores' />);
    const [own, peer] = $$(".oxy-ui-trend-chart__end");
    expect(own.textContent).toBe("This store");
    expect(peer.textContent).toBe("All stores");
    const gap = Math.abs(Number(own.getAttribute("y")) - Number(peer.getAttribute("y")));
    expect(gap).toBeGreaterThanOrEqual(14);
  });

  it("never draws two month labels into each other on a year of months", async () => {
    await render(<TrendChart points={YEAR} storeLabel='North' />);
    const labels = $$("[data-axis='month']");
    expect(labels.length).toBeGreaterThan(2);
    expect(labels.length).toBeLessThan(12);
    const words = labels.map((l) => l.textContent ?? "");
    expect(words[0]).toBe("Jul 2025");
    expect(words[words.length - 1]).toMatch(/^Jun/);
    // The new year is said once, on its first tick.
    expect(words.filter((w) => w.includes("2026"))).toHaveLength(1);
  });

  it("reads a month on hover: crosshair, marks, the toast and the slider's value, gone on leave", async () => {
    await render(
      <TrendChart
        points={YEAR}
        storeLabel='North'
        delta={(p) => `$${(p.observedPct - p.peerPct) * 100} more that month`}
      />
    );
    expect($("[data-crosshair]")).toBeNull();
    await hover(3);
    expect($("[data-crosshair]")).not.toBeNull();
    expect($$("[data-mark='dot']")).toHaveLength(2);
    const toast = $(".oxy-ui-trend-chart__toast") as HTMLElement;
    expect(toast.parentElement).toBe(document.body);
    expect(toast.getAttribute("aria-hidden")).toBe("true");
    expect(toast.textContent).toBe("Oct 2025Comparable 18.0%23.0%$500 more that month");
    expect(said()).toBe("Oct 2025 — North 23.0%, comparable 18.0% · $500 more that month");
    expect(now()).toBe("3");
    await fire(plot(), new MouseEvent("mouseout", { bubbles: true, relatedTarget: document.body }));
    expect($("[data-crosshair]")).toBeNull();
    expect($(".oxy-ui-trend-chart__toast")).toBeNull();
    expect(now()).toBe("11");
  });

  it("reads by keyboard: a focus stop, arrows step the months, Home, End and Escape", async () => {
    await render(<TrendChart points={YEAR} storeLabel='North' />);
    plot().focus();
    expect(document.activeElement).toBe(plot());
    expect(plot().getAttribute("tabindex")).toBe("0");
    expect(plot().className).toContain("oxy-ui-focus");
    expect($("[data-crosshair]")).toBeNull();
    await press("ArrowLeft");
    expect(now()).toBe("11");
    expect(said()).toMatch(/^Jun 2026 — North 31\.0%/);
    expect($("[data-crosshair]")).not.toBeNull();
    await press("ArrowLeft");
    expect(now()).toBe("10");
    expect(said()).toMatch(/^May 2026/);
    await press("ArrowRight");
    await press("ArrowRight");
    expect(now()).toBe("11");
    await press("Home");
    expect(now()).toBe("0");
    expect(said()).toMatch(/^Jul 2025/);
    await press("ArrowLeft");
    expect(now()).toBe("0");
    await press("End");
    expect(now()).toBe("11");
    await press("Escape");
    expect($("[data-crosshair]")).toBeNull();
    expect($(".oxy-ui-trend-chart__toast")).toBeNull();
  });

  it("on a phone reads a month by tap, in a line under the chart", async () => {
    await render(<TrendChart points={TWO} storeLabel='North' fluid unit='stars' />);
    // Drawn at its measured width (a phone sheet's 320 before the first measurement), the same
    // 178 high, the same left pad and month band.
    expect(svg().getAttribute("viewBox")).toBe("0 0 320 178");
    expect($("path[data-series='observed']")?.getAttribute("transform")).toBe("translate(40,14)");
    expect($("[data-axis='month']")?.getAttribute("y")).toBe("172");
    const readout = $(".oxy-ui-trend-chart__readout") as HTMLElement;
    expect(readout.textContent).toBe("Tap a month to compare.");
    await fire($("[data-hit='0']"), new MouseEvent("click", { bubbles: true }));
    expect(readout.textContent).toBe("Jan 2026 — North 10.00, comparable 20.00");
    expect($(".oxy-ui-trend-chart__toast")).toBeNull();
  });

  it("takes the subject's colour from its tone and the marks the app picks", async () => {
    await render(
      <TrendChart points={TWO} storeLabel='North' tone='error' shape='bars' point='square' />
    );
    expect($$("[data-mark='bar']").map((b) => b.getAttribute("fill"))).toEqual([
      "var(--status-error-series)",
      "var(--status-error-series)"
    ]);
    expect($("path[data-series='observed']")).toBeNull();
    expect($("rect[data-mark='square']")?.getAttribute("fill")).toBe("var(--status-error-series)");
    expect($("path[data-series='peer']")?.getAttribute("stroke")).toBe("var(--trend-peer)");
  });

  it("paints only theme tokens, hovered included", async () => {
    await render(<TrendChart points={YEAR} storeLabel='North' tone='warning' />);
    await hover(2);
    const { tokens, off } = paints(svg());
    expect(off).toEqual([]);
    expect([...tokens].sort()).toEqual([
      "--border-strong",
      "--border-subtle",
      "--oxy-ui-chart-ground",
      "--status-warning-series",
      "--trend-peer"
    ]);
  });
});
