// Started from pokehouse-oxy `apps/watchlist/src/lib/trend.ts:54-116` (`trendGeometry`, INVENTORY
// section 3: Watchlist's trend code is ahead of the reference's and tested). Kept: one y-scale
// spanning BOTH series (two scales over one comparison chart would let a line that is below the
// other appear above it), a flat series or a single point drawn along the middle rather than
// dividing by zero, the area as the subject's own path closed to the floor. New: the scale can be
// handed in (`domain`), the point positions come back for the marks, and a tone picks the
// subject's colour from the theme's series tokens. Not here: everything else in `trend.ts` (the
// shape of a run, its months and its dollars), which is the app's arithmetic and reaches the
// charts as props.
//
// 2.31.0: a point's comparison is optional (`SparkPoint`), so a sparkline can be one series, and
// the x positions can be handed in (`at`: a numeric key per point, such as a timestamp, so points
// sit by when they were read rather than by their index).

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

/** A sparkline's point: the subject, and a comparison only where there is one. A `TrendPoint`
 *  is one, so every two-series chart's data fits. */
export interface SparkPoint {
  /** The point's name: `YYYY-MM` for a month, or any key (`sparkPoints` numbers them). */
  period: string;
  /** The subject's value: the line the chart is about (a count or a price as well as a percent). */
  observedPct: number;
  /** The comparison's value, if the chart has one. */
  peerPct?: number | null;
}

/** One series of plain values as sparkline points, named by `periods` or by their index. */
export function sparkPoints(values: readonly number[], periods?: readonly string[]): SparkPoint[] {
  return values.map((v, i) => ({ period: periods?.[i] ?? String(i), observedPct: v }));
}

/** Whether a point has a comparison to draw. */
function hasPeer(p: SparkPoint): p is SparkPoint & { peerPct: number } {
  return typeof p.peerPct === "number" && Number.isFinite(p.peerPct);
}

export interface XY {
  x: number;
  y: number;
}

export interface SeriesGeometry {
  observedPath: string;
  /** `observedPath` closed down to the floor, for an area fill under the subject's line. */
  observedArea: string;
  /** Empty when no point has a comparison. */
  peerPath: string;
  observed: XY[];
  /** The points that have a comparison (every point, for a `TrendPoint` series). */
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
 * y-scale: `domain` when given, else both series' own extent. A point with no comparison is a gap
 * in the comparison's line (none at all: no line). `at`, given, places each point by its own key
 * (a timestamp) across the width, rather than evenly by index.
 */
export function seriesGeometry(
  points: readonly SparkPoint[],
  width: number,
  height: number,
  pad = 2,
  domain?: readonly [number, number],
  at?: readonly number[]
): SeriesGeometry {
  if (points.length === 0) return EMPTY;
  const values = points.flatMap((p) => (hasPeer(p) ? [p.observedPct, p.peerPct] : [p.observedPct]));
  const min = domain ? domain[0] : Math.min(...values);
  const max = domain ? domain[1] : Math.max(...values);
  const range = max - min;
  const innerH = height - pad * 2;
  const keyed = at && at.length === points.length ? at : null;
  const k0 = keyed ? Math.min(...keyed) : 0;
  const k1 = keyed ? Math.max(...keyed) : 0;
  const x = (i: number) =>
    keyed
      ? k1 === k0
        ? width / 2
        : pad + ((keyed[i] - k0) * (width - pad * 2)) / (k1 - k0)
      : points.length === 1
        ? width / 2
        : pad + (i * (width - pad * 2)) / (points.length - 1);
  const y = (v: number) => (range === 0 ? height / 2 : pad + innerH - ((v - min) / range) * innerH);
  const xy = (i: number, v: number) => ({ x: x(i), y: y(v) });
  const move = (p: XY) => `M${p.x.toFixed(2)},${p.y.toFixed(2)}`;
  const line = (p: XY) => `L${p.x.toFixed(2)},${p.y.toFixed(2)}`;
  const path = (xys: XY[]) => xys.map((p, i) => (i === 0 ? move(p) : line(p))).join(" ");
  const observed = points.map((p, i) => xy(i, p.observedPct));
  const peer: XY[] = [];
  const peerParts: string[] = [];
  points.forEach((p, i) => {
    if (!hasPeer(p)) return;
    const pt = xy(i, p.peerPct);
    // A new stroke after a point with no comparison: the gap shows as a gap.
    peerParts.push(i > 0 && hasPeer(points[i - 1]) ? line(pt) : move(pt));
    peer.push(pt);
  });
  const observedPath = path(observed);
  const last = observed.length - 1;
  return {
    observedPath,
    observedArea: `${observedPath} L${x(last).toFixed(2)},${height} L${x(0).toFixed(2)},${height} Z`,
    peerPath: peerParts.join(" "),
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
