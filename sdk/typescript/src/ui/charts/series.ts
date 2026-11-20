// Started from pokehouse-oxy `apps/watchlist/src/lib/trend.ts:54-116` (`trendGeometry`, INVENTORY
// section 3: Watchlist's trend code is ahead of the reference's and tested). Kept: one y-scale
// spanning BOTH series (two scales over one comparison chart would let a line that is below the
// other appear above it), a flat series or a single point drawn along the middle rather than
// dividing by zero, the area as the subject's own path closed to the floor. New: the scale can be
// handed in (`domain`), the point positions come back for the marks, and a tone picks the
// subject's colour from the theme's series tokens. Not here: everything else in `trend.ts` (the
// shape of a run, its months and its dollars), which is the app's arithmetic and reaches the
// charts as props.

import type { StatusTone } from "../tone";

/** One month of a two-series trend: the subject and what it is read against. */
export interface TrendPoint {
  /** `YYYY-MM`. */
  period: string;
  /** The subject's value: the line the chart is about. */
  observedPct: number;
  /** The comparison's value: the recessive line. */
  peerPct: number;
}

export interface XY {
  x: number;
  y: number;
}

export interface SeriesGeometry {
  observedPath: string;
  /** `observedPath` closed down to the floor, for an area fill under the subject's line. */
  observedArea: string;
  peerPath: string;
  observed: XY[];
  peer: XY[];
  /** The subject's last point, for the end mark. */
  lastPoint: XY | null;
  min: number;
  max: number;
  /** A value's height on this scale. */
  y: (value: number) => number;
}

const EMPTY: SeriesGeometry = {
  observedPath: "",
  observedArea: "",
  peerPath: "",
  observed: [],
  peer: [],
  lastPoint: null,
  min: 0,
  max: 0,
  y: () => 0
};

/**
 * The two series as SVG paths in a `width` x `height` box, `pad` in from every edge, on one
 * y-scale: `domain` when given, else both series' own extent.
 */
export function seriesGeometry(
  points: readonly TrendPoint[],
  width: number,
  height: number,
  pad = 2,
  domain?: readonly [number, number]
): SeriesGeometry {
  if (points.length === 0) return EMPTY;
  const values = points.flatMap((p) => [p.observedPct, p.peerPct]);
  const min = domain ? domain[0] : Math.min(...values);
  const max = domain ? domain[1] : Math.max(...values);
  const range = max - min;
  const innerH = height - pad * 2;
  const x = (i: number) =>
    points.length === 1 ? width / 2 : pad + (i * (width - pad * 2)) / (points.length - 1);
  const y = (v: number) => (range === 0 ? height / 2 : pad + innerH - ((v - min) / range) * innerH);
  const at = (pick: (p: TrendPoint) => number) =>
    points.map((p, i) => ({ x: x(i), y: y(pick(p)) }));
  const path = (xys: XY[]) =>
    xys.map((p, i) => `${i === 0 ? "M" : "L"}${p.x.toFixed(2)},${p.y.toFixed(2)}`).join(" ");
  const observed = at((p) => p.observedPct);
  const peer = at((p) => p.peerPct);
  const observedPath = path(observed);
  const last = observed.length - 1;
  return {
    observedPath,
    observedArea: `${observedPath} L${x(last).toFixed(2)},${height} L${x(0).toFixed(2)},${height} Z`,
    peerPath: path(peer),
    observed,
    peer,
    lastPoint: observed[last],
    min,
    max,
    y
  };
}

const SERIES: Record<StatusTone, string> = {
  success: "var(--status-success-series)",
  warning: "var(--status-warning-series)",
  error: "var(--status-error-series)",
  info: "var(--status-info-series)",
  neutral: "var(--status-neutral-series)"
};

/**
 * The subject's colour: the series cut of its status where it has one (a flagged measure draws
 * its own line in the hue it was flagged with, so the chart and the cell that sent the reader
 * here are visibly the same claim), the theme's `--trend-store` where it has none. The comparison
 * is always the neutral `--trend-peer`.
 */
export function seriesColour(tone: StatusTone | null | undefined): string {
  return tone ? SERIES[tone] : "var(--trend-store)";
}

/** How the subject's series is drawn (decision 38b: a chart's marks keep the app's shape). */
export type SeriesShape = "line" | "bars";
