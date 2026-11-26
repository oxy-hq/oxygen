import { cn } from "@/libs/shadcn/utils";

export interface AdminTrendPoint {
  day: string;
  /** The bar's height. */
  value: number;
  /** Shown on hover, already formatted. */
  detail: string;
  /** Draws the bar in the warning tone — a day with failures, say. */
  flagged?: boolean;
  /**
   * The same day one window earlier, drawn as a tick across the bar so a jump
   * is read against a baseline rather than against the chart's own peak.
   * `undefined` draws no tick.
   */
  baseline?: number;
}

/**
 * A daily series as bars, so a regression is dated rather than merely present.
 *
 * SVG-free and library-free, on the cockpit's own aesthetic. It replaced
 * `LlmCostSection/CostBars`, which did the same job for one caller and drew a
 * zero day as nothing. Two details here are not decoration:
 *
 * - **A zero day draws a faint baseline tick, not nothing.** A gap in a bar
 *   chart reads as missing data; a tick reads as a measured zero, which is
 *   what it is.
 * - **A non-zero day never draws thinner than a few pixels.** Scaled honestly
 *   against a peak twenty times its size, a real day of traffic rounds to
 *   nothing and the chart says it did not happen.
 */
export const AdminTrendBars = ({
  points,
  emptyLabel,
  testId,
  className,
  baselineLabel
}: {
  points: AdminTrendPoint[];
  emptyLabel: string;
  testId: string;
  className?: string;
  /** Names the baseline ticks in the footer, when the points carry them. */
  baselineLabel?: string;
}) => {
  if (points.length === 0) {
    return (
      <div
        data-testid={`${testId}-empty`}
        className={cn(
          "flex h-24 items-center justify-center border border-border-subtle border-dashed text-muted-foreground text-xs",
          className
        )}
      >
        {emptyLabel}
      </div>
    );
  }

  // One scale for bars and ticks, so a tick above its bar means "less than
  // the same day last time" in the same units.
  const max = Math.max(...points.map((p) => Math.max(p.value, p.baseline ?? 0)), Number.EPSILON);
  const peak = points.reduce((a, b) => (b.value > a.value ? b : a), points[0]);
  const compared = points.some((p) => p.baseline !== undefined);

  return (
    <div className={cn("space-y-1.5", className)} data-testid={testId}>
      <div className='flex h-24 items-end gap-px'>
        {points.map((p) => (
          <div
            key={p.day}
            className='group relative flex h-full flex-1 items-end'
            title={`${p.day} · ${p.detail}`}
          >
            <div
              className={cn(
                "w-full rounded-t-sm transition-colors",
                p.value <= 0
                  ? "bg-muted-foreground/20"
                  : p.flagged
                    ? "bg-warning/70 group-hover:bg-warning"
                    : // Lighter under a baseline, so the tick reads across it.
                      compared
                      ? "bg-muted-foreground/40 group-hover:bg-muted-foreground/60"
                      : "bg-primary/70 group-hover:bg-primary"
              )}
              style={{ height: `${Math.max((p.value / max) * 100, p.value > 0 ? 4 : 1.5)}%` }}
            />
            {p.baseline !== undefined && p.baseline > 0 ? (
              <div
                aria-hidden
                className='pointer-events-none absolute inset-x-0 h-0.5 rounded-full bg-foreground'
                style={{ bottom: `calc(${(p.baseline / max) * 100}% - 1px)` }}
              />
            ) : null}
          </div>
        ))}
      </div>
      <div className='flex items-center justify-between font-mono text-[10px] text-muted-foreground tabular-nums'>
        <span>{points[0].day}</span>
        <span className='flex items-center gap-3'>
          <span className='text-foreground/70'>peak {peak.detail}</span>
          {compared && baselineLabel ? (
            <span className='flex items-center gap-1 font-sans'>
              <span aria-hidden className='inline-block h-0.5 w-3 rounded-full bg-foreground' />
              {baselineLabel}
            </span>
          ) : null}
        </span>
        <span>{points[points.length - 1].day}</span>
      </div>
    </div>
  );
};
