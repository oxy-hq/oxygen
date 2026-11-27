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
// 2.32.0 (the kept pieces, decision 99), each opt-in, so a sparkline that passes none of it is
// drawn as before:
//   - `domain`: a fixed y scale, so lines drawn side by side compare by height (Accounts Insight's
//     shared 0..max over every customer; a cold-chain sensor's band plus 2°F).
//   - `band`: a shaded range behind the line with dashed edges (a safe range), in the neutral
//     comparison grey (`--trend-peer`): the theme has no "safe range" colour.
//   - `reference`: a dashed line at one value (a mean, a target), in the same grey; `{ value,
//     from }` starts it at a point (Accounts Insight's four-week mean, from its window's start).
//   - `baseline`: a rule at the scale's floor (Accounts Insight's cell foot).
//   - `endTone`: the end mark in a status's series colour, apart from the line's (a direction).
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
  type SeriesGeometry,
  type SeriesPad,
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
  /** A fixed y scale `[min, max]`, so lines side by side compare by height. Default the series'
   *  own extent. */
  domain?: readonly [number, number];
  /** A shaded range `[low, high]` behind the line, with dashed edges (a safe or target range). */
  band?: readonly [number, number] | null;
  /** A dashed line at one value (a mean, a target); `from` starts it at that point's index
   *  (half a step before it) rather than at the left edge. */
  reference?: number | SparkReference | null;
  /** A rule at the scale's floor. Default off. */
  baseline?: boolean;
  /** The end mark's colour as a status (a direction), apart from the line's. Default the line's. */
  endTone?: StatusTone | null;
}

/** A dashed reference line: its value, and the point it starts at. */
export interface SparkReference {
  value: number;
  /** The index of the point the line starts at. Default 0 (the left edge). */
  from?: number;
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

/**
 * The series' points for a shape. A bar takes one `step` of the box's width, its point at the
 * step's centre: the points run half a step in from the 2px pad, so the first and last bars stay
 * inside the box at any width (a line's points run out to the pad itself). The end mark, the
 * readout and a reference's start read the same points, so they stay on the bars.
 */
function shapeGeometry<P extends SparkPoint>(
  points: readonly P[],
  box: { width: number; height: number; step: number },
  shape: SeriesShape,
  domain?: readonly [number, number],
  x?: (point: P) => number
): SeriesGeometry {
  const pad: SeriesPad = shape === "bars" ? { x: 2 + box.step / 2, y: 2 } : 2;
  return seriesGeometry(points, box.width, box.height, pad, domain, x ? points.map(x) : undefined);
}

/** A series under two points: a dash, named and titled with why. */
function EmptySpark({
  label,
  emptyText,
  width,
  height,
  fluid,
  className
}: {
  label: string;
  emptyText: string;
  width: number;
  height: number;
  fluid: boolean;
  className?: string;
}) {
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
  className,
  ...guides
}: SparklineProps<P>) {
  // Gradients live in the document, not the element: two sparklines with one id would share a
  // fill, and every row has its own tone.
  const fillId = `oxy-spark-${useId()}`;
  const tipId = useId();
  const svgRef = useRef<SVGSVGElement>(null);
  const measured = useWidth(svgRef, fluid && points.length >= 2);
  const [at, setAt] = useState<number | null>(null);
  if (points.length < 2) {
    return <EmptySpark {...{ label, emptyText, width, height, fluid, className }} />;
  }
  const Wd = fluid ? (measured ?? width) : width;
  const colour = color ?? seriesColour(tone);
  const top = areaOpacity ?? (tone ? (AREA[tone] ?? 0.16) : 0.16);
  const step = (Wd - 4) / points.length;
  const g = shapeGeometry(points, { width: Wd, height, step }, shape, guides.domain, x);
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
      {guideMarks({ g, width: Wd, height, ...guides })}
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
        <PointMark
          shape={end}
          x={g.lastPoint.x}
          y={g.lastPoint.y}
          r={3.5}
          colour={endColourOf(guides.endTone, colour)}
        />
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

const GUIDE = "var(--trend-peer)";

/** The end mark's colour: the tone's series cut when one is given, else the line's. */
function endColourOf(endTone: StatusTone | null | undefined, colour: string): string {
  return endTone ? seriesColour(endTone) : colour;
}

interface GuideOptions {
  g: SeriesGeometry;
  width: number;
  height: number;
  band?: SparklineProps["band"];
  reference?: SparklineProps["reference"];
  baseline?: boolean;
}

/** The reference line's height and start, or null when there is none to draw. A flat scale
 *  (every value equal, no domain) puts every value at mid-height, so it would only lie on the
 *  line: none. */
function referenceAt(g: SeriesGeometry, reference: SparklineProps["reference"]) {
  const ref = typeof reference === "number" ? { value: reference } : (reference ?? null);
  if (g.max === g.min || !ref || !Number.isFinite(ref.value)) return null;
  return { y: g.y(ref.value), x: referenceStart(g, ref.from ?? 0) };
}

/** Half a step before the reference's first point, so the line covers that point's slot. */
function referenceStart(g: SeriesGeometry, from: number): number {
  const at = g.observed[from];
  if (from <= 0 || !at) return 0;
  return Math.max(0, at.x - (at.x - g.observed[from - 1].x) / 2);
}

/** The band's top and bottom, or null (none asked for, or a flat scale, where it would only lie
 *  on the line). */
function bandAt(g: SeriesGeometry, band: SparklineProps["band"]) {
  return band && g.max !== g.min ? ([g.y(band[1]), g.y(band[0])] as const) : null;
}

function BandMark({ y, width }: { y: readonly [number, number]; width: number }) {
  return (
    <g data-mark='band'>
      <rect
        x={0}
        y={y[0]}
        width={width}
        height={Math.max(0, y[1] - y[0])}
        fill={GUIDE}
        fillOpacity={0.14}
      />
      {y.map((edge, i) => (
        <line
          // biome-ignore lint/suspicious/noArrayIndexKey: the band's two edges, top then bottom
          key={i}
          x1={0}
          x2={width}
          y1={edge}
          y2={edge}
          stroke={GUIDE}
          strokeWidth={1}
          strokeDasharray='3 3'
        />
      ))}
    </g>
  );
}

/** The band, the floor rule and the reference line, under everything else; null when none is
 *  asked for, so a sparkline without them draws exactly as before. */
function guideMarks({ g, width, height, band, reference, baseline = false }: GuideOptions) {
  const ref = referenceAt(g, reference);
  const bandY = bandAt(g, band);
  // The box's foot (the geometry's 2px pad, plus half the rule), not g.y(g.min): on a flat scale
  // that is mid-height, on the line.
  const floorY = height - 1.5;
  if (!bandY && !ref && !baseline) return null;
  return (
    <>
      {bandY && <BandMark y={bandY} width={width} />}
      {baseline && (
        <line
          x1={0}
          x2={width}
          y1={floorY}
          y2={floorY}
          stroke='var(--border)'
          strokeWidth={1}
          data-mark='baseline'
        />
      )}
      {ref && (
        <line
          x1={ref.x}
          x2={width}
          y1={ref.y}
          y2={ref.y}
          stroke={GUIDE}
          strokeWidth={1}
          strokeDasharray='2 2'
          data-mark='reference'
        />
      )}
    </>
  );
}
