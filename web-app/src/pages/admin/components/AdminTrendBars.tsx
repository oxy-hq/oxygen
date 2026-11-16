import { cn } from "@/libs/shadcn/utils";

export interface AdminTrendPoint {
  day: string;
  /** The bar's height. */
  value: number;
  /** Shown on hover, already formatted. */
  detail: string;
  /** Draws the bar in the warning tone — a day with failures, say. */
  flagged?: boolean;
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
  className
}: {
  points: AdminTrendPoint[];
  emptyLabel: string;
  testId: string;
  className?: string;
}) => {
  if (points.length === 0) {
    return (
      <div
        data-testid={`${testId}-empty`}
        className={cn(
          "flex h-24 items-center justify-center rounded-md border border-border/60 border-dashed text-muted-foreground text-xs",
          className
        )}
      >
        {emptyLabel}
      </div>
    );
  }

  const max = Math.max(...points.map((p) => p.value), Number.EPSILON);
  const peak = points.reduce((a, b) => (b.value > a.value ? b : a), points[0]);

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
                    : "bg-primary/70 group-hover:bg-primary"
              )}
              style={{ height: `${Math.max((p.value / max) * 100, p.value > 0 ? 4 : 1.5)}%` }}
            />
          </div>
        ))}
      </div>
      <div className='flex items-center justify-between font-mono text-[10px] text-muted-foreground tabular-nums'>
        <span>{points[0].day}</span>
        <span className='text-foreground/70'>peak {peak.detail}</span>
        <span>{points[points.length - 1].day}</span>
      </div>
    </div>
  );
};
