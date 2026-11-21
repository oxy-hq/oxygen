// Started from pokehouse-oxy `apps/watchlist/src/components/Sparkline.tsx` (INVENTORY section 3:
// the reference's is the model and Watchlist's the closest of its eight copies, the only one whose
// maths, `lib/trend.ts`, is tested). Kept: 104 x 44, no axes, no ticks, no labels; 2px round-capped
// lines, the subject over the recessive comparison (`--trend-peer`); an area under the subject
// that fades to nothing and grows with its status (error .38, warning .26, else .16); a hollow
// end ring; one gradient id per sparkline; a dash and "Not enough history yet" below two points;
// an accessible name, since the shape is never the only channel. New: the status comes in as a
// `tone` (the app maps its own tiers; `seriesColour`), the fill's strength can be set (Watchlist's
// quieter warning tier, decision 7), and the marks are props (`shape`, `end`; decision 38b).
//
// 2.31.0 (the gap round), each opt-in, so a sparkline that passes none of it is drawn as before:
//   - `width` / `height` and `fluid` (the container's width, measured and drawn 1:1, so the end
//     ring stays round and the line 2px): Command Center's 88x30 desk card and phone width, the
//     staging demo apps' 96x28 and 120x28, Platform Pulse and Roadmap's card-wide line.
//   - one series: a point's `peerPct` is optional (`SparkPoint`, `sparkPoints(values)`), and the
//     scale is the subject's own; `area={false}` draws the line alone; `color` for a series whose
//     colour is not a status.
//   - `x`: points placed by a numeric key (Roadmap's readings, spaced by their dates).
//   - `readout`: oxy-hq/platform-pulse and oxy-hq/oxygen-roadmap `src/components/Sparkline.tsx`,
//     the same reading tool in both. The line is a Tab stop; focus shows the last reading, the
//     arrow keys (and Home / End) step through them, the pointer picks the nearest, and the
//     reading's value and period show in a tooltip over a crosshair. Escape, blur or the pointer
//     leaving puts it away. Its name says every value, so nothing is pointer-only.
//
// Texture, not a chart: it answers one question at a glance (is the gap new, or has it always been
// there) and the expanded view (TrendChart) answers the rest. `readout` is for a card whose line
// IS the detail (a metric tile with no expanded view).

import {
  type KeyboardEvent,
  type PointerEvent,
  type RefObject,
  useId,
  useLayoutEffect,
  useRef,
  useState
} from "react";
import { cx } from "../cx";
import { ui } from "../recipes";
import type { StatusTone } from "../tone";
import { PointMark, type PointShape } from "./marks";
import {
  type SeriesShape,
  type SparkPoint,
  seriesColour,
  seriesGeometry,
  type TrendPoint
} from "./series";

const W = 104;
const H = 44;

/** What a sparkline's reading tool says about a point. */
export interface SparkReadout<P extends SparkPoint = SparkPoint> {
  /** A value as the card prints it: "34.3K", "$1,204". */
  format: (value: number) => string;
  /** The point's period as words: "Week of 3 Nov". Default the point's `period`. */
  periodLabel?: (point: P) => string;
}

export interface SparklineProps<P extends SparkPoint = TrendPoint> {
  points: readonly P[];
  /** The accessible name: what the line is ("North, food cost, 12 months"). */
  label: string;
  /** The status of the measure charted, if it has one: the subject's line takes its hue. */
  tone?: StatusTone | null;
  /** A colour for a series that is not a status (a `var()` of a theme token); wins over `tone`. */
  color?: string;
  /** The area's opacity at its top. Default by tone: error .38, warning .26, otherwise .16. */
  areaOpacity?: number;
  /** The area under the subject's line. Default drawn; `false` for the line alone. */
  area?: boolean;
  /** The subject as a line (with its area) or as bars. Default line. */
  shape?: SeriesShape;
  /** The mark on the subject's last point. Default the hollow ring. */
  end?: PointShape;
  /** The drawn width in px. Default 104. With `fluid`, the width before it is measured. */
  width?: number;
  /** The drawn height in px. Default 44. */
  height?: number;
  /** Fill the container's width: measured, and drawn 1:1 at it. */
  fluid?: boolean;
  /** Place points by a numeric key (a timestamp) rather than evenly by index. */
  x?: (point: P) => number;
  /** Make the line a reading tool: a Tab stop whose readings show in a tooltip. */
  readout?: SparkReadout<P>;
  /** Said for a series under two points. */
  emptyText?: string;
  className?: string;
}

const AREA: Partial<Record<StatusTone, number>> = { error: 0.38, warning: 0.26 };

/** The element's width, kept current while `on`; null until measured (jsdom never measures). */
function useWidth(ref: RefObject<Element | null>, on: boolean): number | null {
  const [width, setWidth] = useState<number | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!on || !el) return;
    const read = () => setWidth(Math.round(el.getBoundingClientRect().width) || null);
    read();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref, on]);
  return width;
}

const STEPS: Record<string, (i: number, last: number) => number> = {
  ArrowLeft: (i) => Math.max(0, i - 1),
  ArrowRight: (i, last) => Math.min(last, i + 1),
  Home: () => 0,
  End: (_i, last) => last
};

export function Sparkline<P extends SparkPoint = TrendPoint>({
  points,
  label,
  tone,
  color,
  areaOpacity,
  area = true,
  shape = "line",
  end = "ring",
  width = W,
  height = H,
  fluid = false,
  x,
  readout,
  emptyText = "Not enough history yet",
  className
}: SparklineProps<P>) {
  // Gradients live in the document, not the element: two sparklines with one id would share a
  // fill, and every row has its own tone.
  const fillId = `oxy-spark-${useId()}`;
  const tipId = useId();
  const svgRef = useRef<SVGSVGElement>(null);
  const measured = useWidth(svgRef, fluid && points.length >= 2);
  const [at, setAt] = useState<number | null>(null);
  if (points.length < 2) {
    const sized = width !== W || height !== H || fluid;
    return (
      <span
        role='img'
        aria-label={`${label}: ${emptyText}`}
        title={emptyText}
        className={cx(
          "oxy-ui-sparkline",
          "oxy-ui-sparkline--empty",
          fluid && "oxy-ui-sparkline--fluid",
          className
        )}
        style={sized ? { width: fluid ? undefined : width, height } : undefined}
      >
        —
      </span>
    );
  }
  const Wd = fluid ? (measured ?? width) : width;
  const colour = color ?? seriesColour(tone);
  const top = areaOpacity ?? (tone ? (AREA[tone] ?? 0.16) : 0.16);
  const g = seriesGeometry(points, Wd, height, 2, undefined, x ? points.map(x) : undefined);
  const step = (Wd - 4) / points.length;
  const last = points.length - 1;
  const shown = readout && at !== null ? Math.min(at, last) : null;

  const nearest = (e: PointerEvent<SVGSVGElement>) => {
    const box = e.currentTarget.getBoundingClientRect();
    const vx = box.width > 0 ? ((e.clientX - box.left) / box.width) * Wd : 0;
    let best = 0;
    for (let i = 1; i < g.observed.length; i++)
      if (Math.abs(g.observed[i].x - vx) < Math.abs(g.observed[best].x - vx)) best = i;
    return best;
  };
  const onKeyDown = (e: KeyboardEvent<SVGSVGElement>) => {
    if (e.key === "Escape" && shown !== null) {
      e.preventDefault();
      setAt(null);
      return;
    }
    const move = STEPS[e.key];
    if (!move) return;
    e.preventDefault();
    setAt((i) => move(i ?? last, last));
  };
  // The name says every reading, so the tool is never the only way to them.
  const name = readout
    ? `${label}: ${points.map((p) => readout.format(p.observedPct)).join(", ")}`
    : label;
  const reading = readout
    ? {
        tabIndex: 0,
        "aria-describedby": shown !== null ? tipId : undefined,
        onFocus: () => setAt(last),
        onBlur: () => setAt(null),
        onKeyDown,
        onPointerMove: (e: PointerEvent<SVGSVGElement>) => setAt(nearest(e)),
        onPointerLeave: () => setAt(null)
      }
    : {};

  const svg = (
    <svg
      ref={svgRef}
      width={fluid ? undefined : Wd}
      height={height}
      viewBox={`0 0 ${Wd} ${height}`}
      role='img'
      aria-label={name}
      {...reading}
      className={cx(
        "oxy-ui-sparkline",
        fluid && "oxy-ui-sparkline--fluid",
        readout && ui.focusRing,
        readout ? undefined : className
      )}
    >
      {shape === "line" && area && (
        <>
          <defs>
            <linearGradient id={fillId} x1='0' y1='0' x2='0' y2='1'>
              <stop offset='0%' stopColor={colour} stopOpacity={top} />
              <stop offset='100%' stopColor={colour} stopOpacity={0} />
            </linearGradient>
          </defs>
          {/* Under both lines, so neither has to fight it. */}
          <path d={g.observedArea} fill={`url(#${fillId})`} stroke='none' />
        </>
      )}
      {shape === "bars" &&
        g.observed.map((p, i) => (
          <rect
            key={points[i].period}
            x={p.x - step * 0.3}
            y={p.y}
            width={step * 0.6}
            height={Math.max(height - p.y, 1)}
            fill={colour}
            data-mark='bar'
          />
        ))}
      {/* The comparison: recessive by design, the reference the subject is read against. */}
      {g.peerPath && (
        <path
          d={g.peerPath}
          fill='none'
          stroke='var(--trend-peer)'
          strokeWidth={2}
          strokeLinecap='round'
          strokeLinejoin='round'
          data-series='peer'
        />
      )}
      {shown !== null && (
        <line
          x1={g.observed[shown].x}
          x2={g.observed[shown].x}
          y1={0}
          y2={height}
          stroke='var(--border-strong)'
          strokeWidth={1}
          data-mark='crosshair'
        />
      )}
      {shape === "line" && (
        <path
          d={g.observedPath}
          fill='none'
          stroke={colour}
          strokeWidth={2}
          strokeLinecap='round'
          strokeLinejoin='round'
          data-series='observed'
        />
      )}
      {/* The series' last point, not one more data point among twelve: hollow by default, its
          ground-coloured centre legible where it crosses the comparison. */}
      {g.lastPoint && (
        <PointMark shape={end} x={g.lastPoint.x} y={g.lastPoint.y} r={3.5} colour={colour} />
      )}
      {shown !== null && shown !== last && (
        <PointMark
          shape='dot'
          x={g.observed[shown].x}
          y={g.observed[shown].y}
          r={3}
          colour={colour}
        />
      )}
    </svg>
  );
  if (!readout) return svg;
  const point = shown !== null ? points[shown] : null;
  return (
    <span
      className={cx(
        "oxy-ui-sparkline__frame",
        fluid && "oxy-ui-sparkline__frame--fluid",
        className
      )}
    >
      {svg}
      {point && shown !== null && (
        <span
          id={tipId}
          role='tooltip'
          className='oxy-ui-sparkline__readout'
          style={{ left: g.observed[shown].x }}
        >
          <span className='oxy-ui-sparkline__readout-value'>
            {readout.format(point.observedPct)}
          </span>{" "}
          <span className='oxy-ui-sparkline__readout-period'>
            {readout.periodLabel ? readout.periodLabel(point) : point.period}
          </span>
        </span>
      )}
    </span>
  );
}
