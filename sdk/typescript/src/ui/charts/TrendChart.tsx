// Started from pokehouse-oxy `apps/watchlist/src/components/TrendChart.tsx` (INVENTORY section 3
// and surprise 3: it carries `labelledIndices`, so month labels never collide, and its test; the
// reference lacks the fix). Kept: the frame (one y-scale for both series, three gridlines and
// their ticks, month labels thinned to fit and anchored inward at the ends, direct end labels
// pushed apart when the lines converge, a crosshair, hit areas far larger than the marks), `fluid`
// for a phone (drawn at its measured width, a tap picks a month, the month's figures in a line
// under the chart), and the reference's hover toast over the point, portalled into an open
// <dialog> or the body. The geometry is the reference's, not Watchlist's (reference-app@81ee8aa
// `src/components/TrendChart.tsx:24-48`; SDK-GAPS entry 7): 560 x 178, the y labels 10 off their
// gridlines, and the month labels in a band of their own under the plot. Its labels are NOT the
// reference's: they hold the type scale's xs step (12px) on the glass at every width, as
// Watchlist's did (decision 92: one type scale, and a label scaled to 14.9px on a 694px desk is
// no step on it). The chart measures the width it is drawn at (`--oxy-ui-trend-chart-units-per-px`
// turns the step into viewBox units) and thins the month labels by that width. Narrower than the
// reference's 560, a desk chart draws 1:1 as `fluid` does, since a 12px label no longer fits the
// reference's room scaled down; at 560 and wider its geometry is the reference's.
// New: the status is a `tone` (`seriesColour`); the readout's arithmetic (Watchlist's "$X more
// that month") arrives as `delta`, its words as `storeLabel` / `lineLabel` / `peerLabel` /
// `caption`, its numbers through `format`; the marks are props (`shape`, `point`; decision 38b);
// a flat series gets a scale around its value instead of one its line does not sit on; and the
// keyboard reads it too, where the reference's was mouse-only: the plot is a slider over the
// months (arrows step them, Home and End, Escape clears), its value the month's readout.
// 2.31.1: no two y labels print the same (`valueAxis`): a scale under two label steps tall took
// one label twice ("25%", "25%"); its labels now take the readout's decimal (a percent chart's
// plot starting 12 further in for them, `PAD_L_FINE`), and one too narrow even for that is
// widened around its middle. A chart whose labels differed draws as before.
//
// The expanded trend: the same two series as the Sparkline, now readable. No legend box: the two
// lines are direct-labelled, the more dependable identity channel. One y-scale for both series:
// a second axis would let the subject's line sit above the comparison while its value is below.

import {
  type CSSProperties,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState
} from "react";
import { createPortal } from "react-dom";
import { cx } from "../cx";
import { ui } from "../recipes";
import type { StatusTone } from "../tone";
import { axisTickLabels, labelledIndices, monthLabel, valueAxis } from "./axis";
import { PointMark, type PointShape } from "./marks";
import { type SeriesShape, seriesColour, seriesGeometry, type TrendPoint } from "./series";

const FIXED_W = 560;
const H = 178;
/** Room for the y labels: one ending 6 from its gridline reads as part of the line, 10 apart it
 *  reads as its value, and the widest ("100%") still starts inside the frame. */
const PAD_L = 40;
/** A percent label with the readout's decimal ("25.4%", "100.0%") is up to 12 wider at 12px than
 *  a whole one: its room, where the axis takes that step (2.31.1). */
const PAD_L_FINE = 52;
/** The air between a y label and the gridline it names. */
const Y_LABEL_GAP = 10;
/** Room for the end labels: "Comparable" at the xs step is 68.5 wide. */
const FIXED_PAD_R = 88;
/** Air over the highest gridline. */
const PAD_Y = 14;
/** The band under the plot the month labels live in: its own, since sharing `PAD_Y` put the
 *  months 4 under the lowest gridline, close enough to read as its caption. */
const PAD_B = 24;
/** The month labels' baseline, down from the bottom of the plot. */
const X_LABEL_DROP = 18;
/** Minimum gap between the two end labels before they read as one smudge. */
const LABEL_MIN_GAP = 14;
/** Before the first measurement in a phone sheet. */
const FLUID_FALLBACK_W = 320;
/** The narrowest drawing, in units, however narrow the box. */
const MIN_W = 240;
const FLUID_PAD_R = 80;

export interface TrendChartProps<P extends TrendPoint = TrendPoint> {
  points: readonly P[];
  /** The subject's name in the month's readout ("North"). */
  storeLabel: string;
  /** The subject line's end label. */
  lineLabel?: string;
  /** What the comparison line actually is; say which group it is. */
  peerLabel?: string;
  /** What the axis is a percentage of ("sales"), for the default caption. */
  shareOf?: string;
  /** What the y-axis is measured in: whole-percent ticks, or a rating's one decimal. */
  unit?: "percent" | "stars";
  /** The eyebrow over the chart, and its accessible name. Default "Share of sales, month by
   *  month" or "Average rating, month by month". */
  caption?: ReactNode;
  /** Phone: drawn at the width it is given, and a tap (not a hover) picks a month. */
  fluid?: boolean;
  /** The status of the measure charted: the subject's line takes its hue. */
  tone?: StatusTone | null;
  /** The month's extra line in the readout ("$1,240 more that month"): the app's arithmetic. */
  delta?: (point: P) => string | null;
  /** A value as the readout prints it. Default "24.0%", or "4.46" for stars. */
  format?: (value: number) => string;
  /** A month's words ("Jul", "Jul 2026"). Default English abbreviations. */
  formatPeriod?: (period: string, withYear: boolean) => string;
  /** The subject as a line (with its area) or as bars. Default line. */
  shape?: SeriesShape;
  /** The end mark and the hovered month's marks. Default a haloed dot. */
  point?: PointShape;
  /** Said for a series under two points. */
  emptyText?: string;
  className?: string;
}

/** The width the element is drawn at, in CSS px, kept current; null until measured, when off, and
 *  wherever there is no layout (a server render, jsdom), where the chart takes its viewBox's own. */
function useDrawnWidth(ref: RefObject<Element | null>, on: boolean): number | null {
  const [width, setWidth] = useState<number | null>(null);
  useEffect(() => {
    const el = ref.current;
    if (!on || !el) return;
    const read = () => setWidth(el.getBoundingClientRect().width || null);
    read();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref, on]);
  return width;
}

/** The labels' font size reads this (charts.css): the xs step times units per px. */
const UNITS_PER_PX = "--oxy-ui-trend-chart-units-per-px";

const STEP_KEYS = new Set(["ArrowLeft", "ArrowRight", "Home", "End", "Escape"]);

export function TrendChart<P extends TrendPoint = TrendPoint>({
  points,
  storeLabel,
  lineLabel = "This store",
  peerLabel = "Comparable",
  shareOf = "sales",
  unit = "percent",
  caption,
  fluid = false,
  tone,
  delta,
  format,
  formatPeriod = monthLabel,
  shape = "line",
  point = "dot",
  emptyText = "Not enough history yet to show a trend.",
  className
}: TrendChartProps<P>) {
  const colour = seriesColour(tone);
  const titleId = useId();
  const areaId = `oxy-trend-${useId()}`;
  const [active, setActive] = useState<number | null>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const enough = points.length >= 2;
  const drawn = useDrawnWidth(svgRef, enough);
  // Phone: the drawn width, 1:1. Desk: the reference's 560 scaled to the width, or 1:1 below it.
  const W = Math.max(
    Math.round(fluid ? (drawn ?? FLUID_FALLBACK_W) : Math.min(FIXED_W, drawn ?? FIXED_W)),
    MIN_W
  );
  // SVG units per CSS px: what holds a label at its px size on the glass. 1 unmeasured.
  const unitsPerPx = drawn ? W / drawn : 1;
  // One scale for both series; a flat one still gets a scale its line sits on (its value in the
  // middle), and no two gridlines print the same label (2.31.1).
  const axis = useMemo(
    () =>
      valueAxis(
        points.flatMap((p) => [p.observedPct, p.peerPct]),
        unit
      ),
    [points, unit]
  );
  const padL = axis.fine && unit === "percent" ? PAD_L_FINE : PAD_L;
  const padR = fluid ? FLUID_PAD_R : FIXED_PAD_R;
  const plotW = W - padL - padR;
  const plotH = H - PAD_Y - PAD_B;
  const g = useMemo(
    () => seriesGeometry(points, plotW, plotH, 0, axis.domain),
    [points, plotW, plotH, axis]
  );
  const fmt = format ?? ((v: number) => (unit === "stars" ? v.toFixed(2) : `${v.toFixed(1)}%`));

  if (!enough) {
    return <p className={cx(ui.note, "oxy-ui-trend-chart__empty", className)}>{emptyText}</p>;
  }

  const n = points.length;
  const xAt = (i: number) => padL + (g.observed[i]?.x ?? 0);
  const yAt = (v: number) => PAD_Y + g.y(v);
  const periods = points.map((p) => p.period);
  // Thinned by the drawn width (Watchlist's): labels sit as close as their 12px widths allow.
  const shown = labelledIndices(periods, plotW, unitsPerPx);
  const tickText = axisTickLabels(periods, shown, formatPeriod);
  const last = points[n - 1];
  const on = active === null ? null : points[active];

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!STEP_KEYS.has(event.key)) return;
    event.preventDefault();
    if (event.key === "Escape") return setActive(null);
    if (event.key === "Home") return setActive(0);
    if (event.key === "End") return setActive(n - 1);
    const from = active ?? n;
    const by = event.key === "ArrowLeft" ? -1 : 1;
    setActive(Math.min(n - 1, Math.max(0, active === null && by > 0 ? n - 1 : from + by)));
  };

  // The month's readout: after its name, the two figures and the app's own line.
  const figures = (p: P) => {
    const extra = delta?.(p);
    return ` — ${storeLabel} ${fmt(p.observedPct)}, ${peerLabel.toLowerCase()} ${fmt(p.peerPct)}${extra ? ` · ${extra}` : ""}`;
  };
  // What the slider says: the month in hand, or the latest, which the end mark draws.
  const said = on ?? last;
  const readout = on && (
    <>
      <span className='oxy-ui-trend-chart__month'>{formatPeriod(on.period, true)}</span>
      {figures(on)}
    </>
  );

  // The end labels: pushed apart just enough to stay separate where the lines converge.
  const a = yAt(last.observedPct);
  const b = yAt(last.peerPct);
  const push = Math.abs(a - b) >= LABEL_MIN_GAP ? 0 : (LABEL_MIN_GAP - Math.abs(a - b)) / 2;
  const ownY = a <= b ? a - push : a + push;
  const peerY = a <= b ? b + push : b - push;
  const bar = (plotW / n) * 0.6;

  return (
    <figure className={cx("oxy-ui-trend-chart", className)}>
      <figcaption id={titleId} className={ui.eyebrow}>
        {caption ??
          `${unit === "stars" ? "Average rating" : `Share of ${shareOf}`}, month by month`}
      </figcaption>
      {/* A slider over the months, so a keyboard and a screen reader read the chart too: the
          drawing is its picture, the month's readout its value. */}
      <div
        role='slider'
        aria-labelledby={titleId}
        aria-orientation='horizontal'
        aria-valuemin={0}
        aria-valuemax={n - 1}
        aria-valuenow={active ?? n - 1}
        aria-valuetext={`${formatPeriod(said.period, true)}${figures(said)}`}
        tabIndex={0}
        className={cx("oxy-ui-trend-chart__plot", ui.focusRing)}
        onMouseLeave={() => setActive(null)}
        onKeyDown={onKeyDown}
        onBlur={() => setActive(null)}
      >
        {/* overflow visible: the month labels sit on the bottom edge and their descenders fall
          just outside the box. */}
        <svg
          ref={svgRef}
          viewBox={`0 0 ${W} ${H}`}
          aria-hidden='true'
          className='oxy-ui-trend-chart__svg'
          style={{ [UNITS_PER_PX]: String(Number(unitsPerPx.toFixed(4))) } as CSSProperties}
        >
          {axis.ticks.map((t, i) => (
            <g key={i}>
              <line
                x1={padL}
                x2={padL + plotW}
                y1={yAt(t)}
                y2={yAt(t)}
                stroke='var(--border-subtle)'
                strokeWidth={1}
              />
              <text
                x={padL - Y_LABEL_GAP}
                y={yAt(t) + 3}
                textAnchor='end'
                className='oxy-ui-trend-chart__tick'
                data-axis='value'
              >
                {axis.labels[i]}
              </text>
            </g>
          ))}

          {/* The first and last months anchor inward, clear of the end labels beyond the plot. */}
          {points.map((p, i) =>
            shown.has(i) ? (
              <text
                key={p.period}
                x={i === 0 ? padL : i === n - 1 ? padL + plotW : xAt(i)}
                y={PAD_Y + plotH + X_LABEL_DROP}
                textAnchor={i === 0 ? "start" : i === n - 1 ? "end" : "middle"}
                className='oxy-ui-trend-chart__tick'
                data-axis='month'
              >
                {tickText.get(i)}
              </text>
            ) : null
          )}

          {shape === "line" && (
            <>
              <defs>
                <linearGradient id={areaId} x1='0' y1='0' x2='0' y2='1'>
                  <stop offset='0%' stopColor={colour} stopOpacity={0.3} />
                  <stop offset='100%' stopColor={colour} stopOpacity={0} />
                </linearGradient>
              </defs>
              <path
                d={g.observedArea}
                transform={`translate(${padL},${PAD_Y})`}
                fill={`url(#${areaId})`}
                stroke='none'
              />
            </>
          )}
          {shape === "bars" &&
            points.map((p, i) => (
              <rect
                key={`bar-${p.period}`}
                x={xAt(i) - bar / 2}
                y={yAt(p.observedPct)}
                width={bar}
                height={Math.max(PAD_Y + plotH - yAt(p.observedPct), 1)}
                fill={colour}
                data-mark='bar'
              />
            ))}
          <path
            d={g.peerPath}
            transform={`translate(${padL},${PAD_Y})`}
            fill='none'
            stroke='var(--trend-peer)'
            strokeWidth={2}
            strokeLinecap='round'
            strokeLinejoin='round'
            data-series='peer'
          />
          {shape === "line" && (
            <path
              d={g.observedPath}
              transform={`translate(${padL},${PAD_Y})`}
              fill='none'
              stroke={colour}
              strokeWidth={2}
              strokeLinecap='round'
              strokeLinejoin='round'
              data-series='observed'
            />
          )}

          <text
            x={padL + plotW + 8}
            y={ownY + 3}
            className='oxy-ui-trend-chart__end oxy-ui-trend-chart__end--own'
          >
            {lineLabel}
          </text>
          <text x={padL + plotW + 8} y={peerY + 3} className='oxy-ui-trend-chart__end'>
            {peerLabel}
          </text>

          {on && active !== null && (
            <line
              x1={xAt(active)}
              x2={xAt(active)}
              y1={PAD_Y}
              y2={PAD_Y + plotH}
              stroke='var(--border-strong)'
              strokeWidth={1}
              data-crosshair=''
            />
          )}
          {points.map((p, i) => (
            <g key={`m-${p.period}`}>
              {active === i && (
                <>
                  <PointMark
                    shape={point}
                    x={xAt(i)}
                    y={yAt(p.peerPct)}
                    r={4}
                    colour='var(--trend-peer)'
                  />
                  <PointMark
                    shape={point}
                    x={xAt(i)}
                    y={yAt(p.observedPct)}
                    r={4}
                    colour={colour}
                  />
                </>
              )}
              {/* A hit area far larger than the mark. */}
              <rect
                x={xAt(i) - plotW / (n * 2) - 2}
                y={0}
                width={plotW / n + 4}
                height={H}
                fill='transparent'
                data-hit={i}
                onMouseEnter={() => setActive(i)}
                onClick={fluid ? () => setActive(i) : undefined}
              />
            </g>
          ))}
          {!on && g.lastPoint && (
            <PointMark
              shape={point}
              x={padL + g.lastPoint.x}
              y={PAD_Y + g.lastPoint.y}
              r={4}
              colour={colour}
            />
          )}
        </svg>
      </div>

      {fluid ? (
        <p className='oxy-ui-trend-chart__readout'>
          {readout || <span className='oxy-ui-trend-chart__prompt'>Tap a month to compare.</span>}
        </p>
      ) : (
        on &&
        active !== null && (
          <HoverToast
            anchor={svgRef.current}
            w={W}
            x={xAt(active)}
            y={yAt(on.observedPct)}
            month={formatPeriod(on.period, true)}
            peer={`${peerLabel} ${fmt(on.peerPct)}`}
            value={fmt(on.observedPct)}
            delta={delta ? delta(on) : null}
          />
        )
      )}
    </figure>
  );
}

/**
 * The month's figures over the point they belong to (the reference's `HoverToast`, by way of
 * Watchlist): `position: fixed` so no ancestor's overflow clips it, portalled into the open
 * <dialog> (which paints in the top layer) or the body. The value in the ink at the xl step, not
 * the series colour (text owes 4.5:1). Hidden from assistive tech: the slider's value says it.
 */
function HoverToast({
  anchor,
  w,
  x,
  y,
  month,
  peer,
  value,
  delta
}: {
  anchor: SVGSVGElement | null;
  w: number;
  x: number;
  y: number;
  month: string;
  peer: string;
  value: string;
  delta: string | null;
}) {
  if (!anchor) return null;
  const r = anchor.getBoundingClientRect();
  const scale = r.width / w || 1;
  // Clamped so a first-month hover near the left edge keeps the toast on screen.
  const left = Math.min(Math.max(r.left + x * scale, 90), window.innerWidth - 90);
  // Above the point, clear of the 4px marker and its 2px halo.
  const top = r.top + y * scale - 14;
  return createPortal(
    <div className='oxy-ui-trend-chart__toast' style={{ left, top }} aria-hidden='true'>
      <div className='oxy-ui-trend-chart__bubble'>
        <p className='oxy-ui-trend-chart__toast-month'>{month}</p>
        <p className='oxy-ui-trend-chart__toast-note'>{peer}</p>
        <p className='oxy-ui-trend-chart__toast-value'>{value}</p>
        {delta ? <p className='oxy-ui-trend-chart__toast-note'>{delta}</p> : null}
      </div>
      <span className='oxy-ui-trend-chart__caret' />
    </div>,
    anchor.closest("dialog") ?? document.body
  );
}
