// @vitest-environment jsdom
//
// 2.32.0 Sparkline `domain`, `band`, `reference`, `baseline` and `endTone` (decision 99: Accounts
// Insight's shared scale, four-week mean, cell foot and direction dot; the cold-chain sensors'
// safe band). Each measured on the drawn SVG. Without them the line is as it was
// (keptPieces.defaults.test.tsx).

import { afterEach, describe, expect, it } from "vitest";
import { $, $$, render } from "../ui.harness";
import { Sparkline, type SparklineProps } from "./Sparkline";
import { type SparkPoint, sparkPoints } from "./series";

afterEach(() => {
  document.body.innerHTML = "";
});

const W = 150;
const H = 36;
/** seriesGeometry's scale: 2px in from top and bottom. */
const yOn = (v: number, min: number, max: number) =>
  2 + (H - 4) - ((v - min) / (max - min)) * (H - 4);
const num = (el: Element | null | undefined, attr: string) => Number(el?.getAttribute(attr));

async function draw(props: Partial<SparklineProps<SparkPoint>>, values = [5, 6, 5, 6, 5]) {
  await render(
    <Sparkline
      label='Active people by week'
      points={sparkPoints(values)}
      area={false}
      end='dot'
      width={W}
      height={H}
      {...props}
    />
  );
}

/** The y of the observed line's first point. */
const firstY = () =>
  Number(/^M[\d.]+,([\d.]+)/.exec($('[data-series="observed"]')?.getAttribute("d") ?? "")?.[1]);

describe("Sparkline domain", () => {
  it("draws on the fixed scale, so a flat 5-6 line sits low on 0..14", async () => {
    await draw({ domain: [0, 14] });
    expect(firstY()).toBeCloseTo(yOn(5, 0, 14), 1);
    document.body.innerHTML = "";
    await draw({});
    expect(firstY()).toBeCloseTo(yOn(5, 5, 6), 1);
  });
});

describe("Sparkline band", () => {
  it("shades the range on the line's scale, with two dashed edges in the comparison grey", async () => {
    await draw({ domain: [30, 50], band: [35, 41] }, [36, 38, 40]);
    const band = $('[data-mark="band"]');
    const rect = band?.querySelector("rect");
    expect(num(rect, "y")).toBeCloseTo(yOn(41, 30, 50), 2);
    expect(num(rect, "y") + num(rect, "height")).toBeCloseTo(yOn(35, 30, 50), 2);
    expect(num(rect, "width")).toBe(W);
    expect(rect?.getAttribute("fill")).toBe("var(--trend-peer)");
    const edges = [...(band?.querySelectorAll("line") ?? [])];
    expect(edges).toHaveLength(2);
    for (const e of edges) expect(e.getAttribute("stroke-dasharray")).toBe("3 3");
    // Under the line: the band comes before it in paint order.
    const svg = document.querySelector("svg") as SVGSVGElement;
    const order = [...svg.children];
    expect(order.indexOf(band as Element)).toBeLessThan(
      order.indexOf($('[data-series="observed"]') as Element)
    );
  });
});

describe("Sparkline reference", () => {
  it("draws a dashed line at the value across the width", async () => {
    await draw({ domain: [0, 14], reference: 7 });
    const ref = $('[data-mark="reference"]');
    expect(num(ref, "y1")).toBeCloseTo(yOn(7, 0, 14), 2);
    expect(num(ref, "x1")).toBe(0);
    expect(num(ref, "x2")).toBe(W);
    expect(ref?.getAttribute("stroke-dasharray")).toBe("2 2");
  });

  it("starts at a point, half a step before it, when given `from`", async () => {
    // Eight weeks: the mean of the four before last starts at week index 3.
    await draw({ domain: [0, 14], reference: { value: 7, from: 3 } }, [5, 6, 7, 8, 9, 10, 11, 4]);
    const step = (W - 4) / 7;
    expect(num($('[data-mark="reference"]'), "x1")).toBeCloseTo(2 + 3 * step - step / 2, 2);
  });

  it("draws nothing for null", async () => {
    await draw({ reference: null });
    expect($('[data-mark="reference"]')).toBeNull();
  });
});

describe("Sparkline baseline", () => {
  it("rules the scale's floor", async () => {
    await draw({ domain: [0, 14], baseline: true });
    const rule = $('[data-mark="baseline"]');
    expect(num(rule, "y1")).toBeCloseTo(yOn(0, 0, 14) + 0.5, 2);
    expect(num(rule, "x2")).toBe(W);
  });

  it("stays at the foot on a flat series, where band and reference are not drawn", async () => {
    // Every value equal and a zero-range domain: the scale is flat and the line sits mid-height.
    await draw({ domain: [0, 0], baseline: true, band: [0, 0], reference: 0 }, [0, 0, 0, 0]);
    expect(num($('[data-mark="baseline"]'), "y1")).toBeCloseTo(H - 1.5, 2);
    expect(firstY()).toBeCloseTo(H / 2, 2);
    expect($('[data-mark="band"]')).toBeNull();
    expect($('[data-mark="reference"]')).toBeNull();
  });
});

describe("Sparkline endTone", () => {
  it("colours the end mark by the tone's series colour, the line keeping its own", async () => {
    await draw({ color: "var(--foreground)", endTone: "error" });
    expect($('[data-series="observed"]')?.getAttribute("stroke")).toBe("var(--foreground)");
    expect($('[data-mark="dot"]')?.getAttribute("fill")).toBe("var(--status-error-series)");
  });

  it("leaves the end mark in the line's colour without one", async () => {
    await draw({ color: "var(--foreground)" });
    expect($('[data-mark="dot"]')?.getAttribute("fill")).toBe("var(--foreground)");
    expect($$("[data-mark]").map((m) => m.getAttribute("data-mark"))).toEqual(["dot"]);
  });
});
