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
// Texture, not a chart: it answers one question at a glance (is the gap new, or has it always been
// there) and the expanded view (TrendChart) answers the rest.

import { useId } from "react";
import { cx } from "../cx";
import type { StatusTone } from "../tone";
import { PointMark, type PointShape } from "./marks";
import { type SeriesShape, seriesColour, seriesGeometry, type TrendPoint } from "./series";

const W = 104;
const H = 44;

export interface SparklineProps<P extends TrendPoint = TrendPoint> {
  points: readonly P[];
  /** The accessible name: what the line is ("North, food cost, 12 months"). */
  label: string;
  /** The status of the measure charted, if it has one: the subject's line takes its hue. */
  tone?: StatusTone | null;
  /** The area's opacity at its top. Default by tone: error .38, warning .26, otherwise .16. */
  areaOpacity?: number;
  /** The subject as a line (with its area) or as bars. Default line. */
  shape?: SeriesShape;
  /** The mark on the subject's last point. Default the hollow ring. */
  end?: PointShape;
  /** Said for a series under two points. */
  emptyText?: string;
  className?: string;
}

const AREA: Partial<Record<StatusTone, number>> = { error: 0.38, warning: 0.26 };

export function Sparkline<P extends TrendPoint = TrendPoint>({
  points,
  label,
  tone,
  areaOpacity,
  shape = "line",
  end = "ring",
  emptyText = "Not enough history yet",
  className
}: SparklineProps<P>) {
  // Gradients live in the document, not the element: two sparklines with one id would share a
  // fill, and every row has its own tone.
  const fillId = `oxy-spark-${useId()}`;
  if (points.length < 2) {
    return (
      <span
        role='img'
        aria-label={`${label}: ${emptyText}`}
        title={emptyText}
        className={cx("oxy-ui-sparkline", "oxy-ui-sparkline--empty", className)}
      >
        —
      </span>
    );
  }
  const colour = seriesColour(tone);
  const top = areaOpacity ?? (tone ? (AREA[tone] ?? 0.16) : 0.16);
  const g = seriesGeometry(points, W, H);
  const step = (W - 4) / points.length;
  return (
    <svg
      width={W}
      height={H}
      viewBox={`0 0 ${W} ${H}`}
      role='img'
      aria-label={label}
      className={cx("oxy-ui-sparkline", className)}
    >
      {shape === "line" && (
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
            height={Math.max(H - p.y, 1)}
            fill={colour}
            data-mark='bar'
          />
        ))}
      {/* The comparison: recessive by design, the reference the subject is read against. */}
      <path
        d={g.peerPath}
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
    </svg>
  );
}
