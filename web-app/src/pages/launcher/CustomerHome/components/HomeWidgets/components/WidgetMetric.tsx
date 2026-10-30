import { ArrowDownRight, ArrowRight, ArrowUpRight } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import type { WidgetMetric as Metric } from "@/types/homeWidgets";

const TREND = {
  up: { Icon: ArrowUpRight, className: "text-success" },
  down: { Icon: ArrowDownRight, className: "text-destructive" },
  flat: { Icon: ArrowRight, className: "text-muted-foreground" }
} as const;

/** A sparkline as an SVG polyline over the points' own range. */
function Spark({ points }: { points: number[] }) {
  if (points.length < 2) return null;
  const min = Math.min(...points);
  const span = Math.max(...points) - min || 1;
  const coords = points
    .map((p, i) => `${(i / (points.length - 1)) * 100},${30 - ((p - min) / span) * 28 - 1}`)
    .join(" ");
  return (
    <svg
      viewBox='0 0 100 30'
      preserveAspectRatio='none'
      aria-hidden
      className='h-8 w-24 shrink-0 text-primary'
    >
      <polyline points={coords} fill='none' stroke='currentColor' strokeWidth='2' />
    </svg>
  );
}

/** A widget's KPI: the number, what it is, how it moved, and a sparkline. */
export function WidgetMetric({ metric, testId }: { metric: Metric; testId: string }) {
  const trend = metric.trend ? TREND[metric.trend] : null;
  return (
    <div className='flex items-end justify-between gap-4' data-testid={testId}>
      <div className='flex min-w-0 flex-col'>
        <span className='font-semibold text-2xl text-card-foreground tabular-nums'>
          {metric.value}
        </span>
        {(metric.label || metric.delta) && (
          <span className='flex items-center gap-1.5 text-muted-foreground text-xs'>
            {metric.label}
            {metric.delta && (
              <span className={cn("inline-flex items-center gap-0.5", trend?.className)}>
                {trend && <trend.Icon className='size-3' aria-hidden />}
                {metric.delta}
              </span>
            )}
          </span>
        )}
      </div>
      <Spark points={metric.spark} />
    </div>
  );
}
