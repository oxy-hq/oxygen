// @vitest-environment jsdom
//
// The in-row trend: its paths for given data on one scale for both series, the middle for a flat
// series, a named dash for empty data and for one point, its accessible name, the subject's
// colour from its tone and the fill growing with it, one gradient per sparkline, the marks an
// app picks, and only theme tokens painted.

import { afterEach, describe, expect, it } from "vitest";
import { $, $$, render } from "../ui.harness";
import { paints } from "./paint.harness";
import { Sparkline } from "./Sparkline";
import { seriesGeometry, type TrendPoint } from "./series";

afterEach(() => {
  document.body.innerHTML = "";
});

const TWO: TrendPoint[] = [
  { period: "2026-01", observedPct: 10, peerPct: 20 },
  { period: "2026-02", observedPct: 30, peerPct: 20 }
];

const svg = () => $("svg") as SVGSVGElement & HTMLElement;
const d = (series: string) => $(`path[data-series='${series}']`)?.getAttribute("d");

describe("Sparkline", () => {
  it("draws both series on one scale across its 104 x 44 box, named for assistive tech", async () => {
    await render(<Sparkline points={TWO} label='North, food cost' />);
    expect(svg().getAttribute("role")).toBe("img");
    expect(svg().getAttribute("aria-label")).toBe("North, food cost");
    expect(svg().getAttribute("viewBox")).toBe("0 0 104 44");
    expect(d("observed")).toBe("M2.00,42.00 L102.00,2.00");
    expect(d("peer")).toBe("M2.00,22.00 L102.00,22.00");
    const area = $$("path").find((p) => p.getAttribute("fill")?.startsWith("url(#"));
    expect(area?.getAttribute("d")).toBe("M2.00,42.00 L102.00,2.00 L102.00,44 L2.00,44 Z");
    // The end mark: a hollow ring on the subject's last point.
    const end = $("[data-mark='ring']") as HTMLElement;
    expect([end.getAttribute("cx"), end.getAttribute("cy")]).toEqual(["102", "2"]);
    expect(end.getAttribute("fill")).toBe("var(--oxy-ui-chart-ground)");
  });

  it("draws a flat series along the middle rather than dividing by zero", async () => {
    const flat = TWO.map((p) => ({ ...p, observedPct: 5, peerPct: 5 }));
    await render(<Sparkline points={flat} label='Flat' />);
    expect(d("observed")).toBe("M2.00,22.00 L102.00,22.00");
    expect(d("peer")).toBe("M2.00,22.00 L102.00,22.00");
  });

  it("says there is not enough history for empty data and for one point, still named", async () => {
    for (const points of [[], TWO.slice(0, 1)]) {
      const page = await render(<Sparkline points={points} label='Harbour, labor' />);
      expect($("svg")).toBeNull();
      const dash = $("[role='img']") as HTMLElement;
      expect(dash.textContent).toBe("—");
      expect(dash.getAttribute("aria-label")).toBe("Harbour, labor: Not enough history yet");
      expect(dash.title).toBe("Not enough history yet");
      await page.unmount();
    }
    expect(seriesGeometry([], 104, 44).observedPath).toBe("");
  });

  it("takes the subject's colour from its tone, the store's line without one", async () => {
    const colours: Record<string, string> = {
      none: "var(--trend-store)",
      error: "var(--status-error-series)",
      warning: "var(--status-warning-series)",
      success: "var(--status-success-series)"
    };
    for (const [tone, colour] of Object.entries(colours)) {
      const page = await render(
        <Sparkline
          points={TWO}
          label='x'
          tone={tone === "none" ? undefined : (tone as "error" | "warning" | "success")}
        />
      );
      expect($("path[data-series='observed']")?.getAttribute("stroke"), tone).toBe(colour);
      expect($("path[data-series='peer']")?.getAttribute("stroke")).toBe("var(--trend-peer)");
      await page.unmount();
    }
  });

  it("grows the area with the status, and takes an app's own strength", async () => {
    const top = () => $("stop")?.getAttribute("stop-opacity");
    let page = await render(<Sparkline points={TWO} label='x' tone='error' />);
    expect(top()).toBe("0.38");
    await page.unmount();
    page = await render(<Sparkline points={TWO} label='x' tone='warning' />);
    expect(top()).toBe("0.26");
    await page.unmount();
    page = await render(<Sparkline points={TWO} label='x' tone='success' />);
    expect(top()).toBe("0.16");
    await page.unmount();
    await render(<Sparkline points={TWO} label='x' tone='warning' areaOpacity={0.16} />);
    expect(top()).toBe("0.16");
  });

  it("gives every sparkline its own gradient", async () => {
    await render(
      <>
        <Sparkline points={TWO} label='a' />
        <Sparkline points={TWO} label='b' tone='error' />
      </>
    );
    const ids = $$("linearGradient").map((g) => g.id);
    expect(new Set(ids).size).toBe(2);
    const fills = $$("path")
      .map((p) => p.getAttribute("fill"))
      .filter((f) => f?.startsWith("url"));
    expect(fills).toEqual(ids.map((id) => `url(#${id})`));
  });

  it("draws the marks an app picks: bars for the subject, another end mark or none", async () => {
    let page = await render(<Sparkline points={TWO} label='x' shape='bars' end='square' />);
    expect($$("[data-mark='bar']")).toHaveLength(2);
    expect($("path[data-series='observed']")).toBeNull();
    expect($("linearGradient")).toBeNull();
    expect($("rect[data-mark='square']")).not.toBeNull();
    await page.unmount();
    page = await render(<Sparkline points={TWO} label='x' end='none' />);
    expect($("[data-mark]")).toBeNull();
  });

  it("paints only theme tokens", async () => {
    await render(<Sparkline points={TWO} label='x' tone='warning' />);
    const { tokens, off } = paints(svg());
    expect(off).toEqual([]);
    expect([...tokens].sort()).toEqual([
      "--oxy-ui-chart-ground",
      "--status-warning-series",
      "--trend-peer"
    ]);
  });
});
