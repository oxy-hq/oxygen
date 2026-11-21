// @vitest-environment node
//
// The chart renders without a DOM (a screen rendered to markup on a server, or in a node test):
// with nothing laid out to measure, it draws at its viewBox's own size, one SVG unit per CSS px,
// so the labels' 12px rule has its fallback and nothing reads a layout (decision 92).

import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { TrendChart } from "./TrendChart";

const TWO = [
  { period: "2026-01", observedPct: 10, peerPct: 20 },
  { period: "2026-02", observedPct: 30, peerPct: 20 }
];

describe("TrendChart without a DOM", () => {
  it("renders to markup in node at its viewBox's own size, desk and phone", () => {
    expect(typeof (globalThis as { document?: unknown }).document).toBe("undefined");
    const desk = renderToStaticMarkup(<TrendChart points={TWO} storeLabel='North' />);
    expect(desk).toContain('viewBox="0 0 560 178"');
    expect(desk).toContain(`style="--oxy-ui-trend-chart-units-per-px:1"`);
    const phone = renderToStaticMarkup(<TrendChart points={TWO} storeLabel='North' fluid />);
    expect(phone).toContain('viewBox="0 0 320 178"');
    expect(phone).toContain(`style="--oxy-ui-trend-chart-units-per-px:1"`);
  });
});
