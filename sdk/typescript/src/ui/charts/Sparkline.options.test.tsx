// @vitest-environment jsdom
//
// The sparkline's 2.31.0 options, each opt-in: a caller's size, the container's width (measured,
// drawn 1:1), one series with no comparison, the line without its area, a colour that is not a
// status, points placed by a key (a date), and the reading tool (a Tab stop whose readings show in
// a tooltip, by keys and by the pointer). And without any of them, the same sparkline as before.

import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { $, $$, fire, press, render } from "../ui.harness";
import { Sparkline } from "./Sparkline";
import { type SparkPoint, seriesGeometry, sparkPoints, type TrendPoint } from "./series";

afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

const TWO: TrendPoint[] = [
  { period: "2026-01", observedPct: 10, peerPct: 20 },
  { period: "2026-02", observedPct: 30, peerPct: 20 }
];
const ONE_SERIES = sparkPoints([4, 8, 6], ["Week 1", "Week 2", "Week 3"]);

const svg = () => $("svg.oxy-ui-sparkline") as SVGSVGElement & HTMLElement;
const d = (series: string) => $(`path[data-series='${series}']`)?.getAttribute("d");
const tip = () => $("[role='tooltip']");

const box = (width: number, height = 44) =>
  ({
    left: 0,
    top: 0,
    right: width,
    bottom: height,
    width,
    height,
    x: 0,
    y: 0,
    toJSON: () => ({})
  }) as DOMRect;

/** The pointer at `clientX` over a line drawn `width` wide from x 0. */
function pointerAt(el: Element, clientX: number, width: number) {
  vi.spyOn(el, "getBoundingClientRect").mockReturnValue(box(width));
  return fire(el, new PointerEvent("pointermove", { bubbles: true, clientX }));
}

describe("Sparkline options", () => {
  it("draws the same sparkline as before when none is passed: no frame, no Tab stop", async () => {
    await render(<Sparkline points={TWO} label='North, food cost' className='x' />);
    expect(svg().parentElement?.classList.contains("oxy-ui-sparkline__frame")).toBe(false);
    expect(svg().hasAttribute("tabindex")).toBe(false);
    expect(svg().getAttribute("width")).toBe("104");
    expect(svg().classList).toContain("x");
  });

  it("draws at the caller's width and height", async () => {
    await render(<Sparkline points={TWO} label='x' width={88} height={30} />);
    expect(svg().getAttribute("viewBox")).toBe("0 0 88 30");
    expect([svg().getAttribute("width"), svg().getAttribute("height")]).toEqual(["88", "30"]);
    expect(d("observed")).toBe("M2.00,28.00 L86.00,2.00");
  });

  it("draws one series with no comparison, on the subject's own scale", async () => {
    await render(<Sparkline points={ONE_SERIES} label='Net sales by week' />);
    expect($("path[data-series='peer']")).toBeNull();
    // 4..8 spans the whole height: the low point on the floor, the high one at the top.
    expect(d("observed")).toBe("M2.00,42.00 L52.00,2.00 L102.00,22.00");
    expect(ONE_SERIES[1]).toEqual({ period: "Week 2", observedPct: 8 });
    expect(sparkPoints([1, 2])).toEqual([
      { period: "0", observedPct: 1 },
      { period: "1", observedPct: 2 }
    ]);
  });

  it("leaves a gap in the comparison where a point has none", () => {
    const pts: SparkPoint[] = [
      { period: "a", observedPct: 0, peerPct: 0 },
      { period: "b", observedPct: 0, peerPct: null },
      { period: "c", observedPct: 0, peerPct: 0 }
    ];
    expect(seriesGeometry(pts, 104, 44).peerPath).toBe("M2.00,22.00 M102.00,22.00");
  });

  it("draws the line without its area, in a colour that is not a status", async () => {
    await render(
      <Sparkline points={TWO} label='x' area={false} tone='error' color='var(--chart-cat-3)' />
    );
    expect($("linearGradient")).toBeNull();
    expect($$("path").some((p) => p.getAttribute("fill")?.startsWith("url"))).toBe(false);
    expect($("path[data-series='observed']")?.getAttribute("stroke")).toBe("var(--chart-cat-3)");
  });

  it("places points by a key, so readings sit by when they were read", async () => {
    const dated = [
      { period: "a", at: 0, observedPct: 1 },
      { period: "b", at: 1, observedPct: 2 },
      { period: "c", at: 4, observedPct: 3 }
    ];
    await render(<Sparkline points={dated} label='x' x={(p) => p.at} />);
    // 0, 1, 4 across 2..102: the second point a quarter of the way, not half.
    expect(d("observed")).toBe("M2.00,42.00 L27.00,22.00 L102.00,2.00");
  });

  it("fluid: measures its container's width and draws 1:1 at it", async () => {
    vi.spyOn(SVGElement.prototype, "getBoundingClientRect").mockReturnValue(box(240));
    await render(<Sparkline points={TWO} label='x' fluid height={36} />);
    expect(svg().getAttribute("viewBox")).toBe("0 0 240 36");
    expect(svg().hasAttribute("width")).toBe(false);
    expect(svg().classList).toContain("oxy-ui-sparkline--fluid");
    expect(d("observed")).toBe("M2.00,34.00 L238.00,2.00");
  });

  it("readout: a Tab stop named with every value; focus shows the last reading", async () => {
    await render(
      <Sparkline
        points={ONE_SERIES}
        label='Signups'
        readout={{ format: (v) => `${v} signups`, periodLabel: (p) => `${p.period} (wk)` }}
      />
    );
    expect(svg().getAttribute("tabindex")).toBe("0");
    expect(svg().getAttribute("aria-label")).toBe("Signups: 4 signups, 8 signups, 6 signups");
    expect(tip()).toBeNull();
    await act(async () => svg().focus());
    expect(tip()?.textContent).toBe("6 signups Week 3 (wk)");
    expect(svg().getAttribute("aria-describedby")).toBe(tip()?.id);
    expect($("[data-mark='crosshair']")?.getAttribute("x1")).toBe("102");
  });

  it("readout: the arrows, Home and End step through readings; Escape and blur put it away", async () => {
    await render(<Sparkline points={ONE_SERIES} label='Signups' readout={{ format: String }} />);
    await act(async () => svg().focus());
    await press("ArrowLeft");
    expect(tip()?.textContent).toBe("8 Week 2");
    // Away from the last point, the reading gets its own dot.
    expect($("circle[data-mark='dot']")).not.toBeNull();
    await press("Home");
    expect(tip()?.textContent).toBe("4 Week 1");
    await press("ArrowLeft");
    expect(tip()?.textContent).toBe("4 Week 1");
    await press("End");
    expect(tip()?.textContent).toBe("6 Week 3");
    await press("Escape");
    expect(tip()).toBeNull();
    await press("ArrowLeft");
    expect(tip()?.textContent).toBe("8 Week 2");
    await act(async () => svg().blur());
    expect(tip()).toBeNull();
  });

  it("readout: the pointer picks the nearest reading, and leaving puts it away", async () => {
    await render(<Sparkline points={ONE_SERIES} label='Signups' readout={{ format: String }} />);
    // Drawn 208px wide for the 104 viewBox: 60px on screen is 30 in the box, nearest Week 2.
    await pointerAt(svg(), 60, 208);
    expect(tip()?.textContent).toBe("8 Week 2");
    expect((tip() as HTMLElement).style.left).toBe("52px");
    await pointerAt(svg(), 0, 208);
    expect(tip()?.textContent).toBe("4 Week 1");
    // React draws its leave from the pointer going out to something outside the line.
    await fire(
      svg(),
      new PointerEvent("pointerout", { bubbles: true, relatedTarget: document.body })
    );
    expect(tip()).toBeNull();
  });

  it("keeps the empty dash at the caller's size", async () => {
    await render(<Sparkline points={TWO.slice(0, 1)} label='x' width={88} height={30} />);
    const dash = $("[role='img']") as HTMLElement;
    expect([dash.style.width, dash.style.height]).toEqual(["88px", "30px"]);
  });
});
