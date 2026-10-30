import { cn } from "@/libs/shadcn/utils";

/** The ring's circle: a circumference of 100 makes the dash length the percentage. */
const RADIUS = 100 / (2 * Math.PI);

/**
 * "N of M" as a ring beside the headline on a one-column card, where a bar's
 * own line would cost a row, or as a bar across a wider one. Both are the same
 * progressbar to assistive tech.
 */
export function WidgetProgress({
  done,
  total,
  variant,
  testId
}: {
  done: number;
  total: number;
  variant: "ring" | "bar";
  testId: string;
}) {
  const percent = total > 0 ? Math.min(100, Math.max(0, (done / total) * 100)) : 0;
  const a11y = {
    role: "progressbar",
    "aria-valuemin": 0,
    "aria-valuemax": total,
    "aria-valuenow": done,
    "aria-label": `${done} of ${total}`,
    "data-testid": testId,
    "data-variant": variant
  } as const;

  if (variant === "bar") {
    return (
      <div {...a11y} className='h-1.5 overflow-hidden rounded-full bg-muted'>
        <div className='h-full rounded-full bg-primary' style={{ width: `${percent}%` }} />
      </div>
    );
  }
  return (
    <div {...a11y} className='relative size-12 shrink-0'>
      <svg viewBox='0 0 36 36' className='size-full -rotate-90' aria-hidden>
        <circle cx='18' cy='18' r={RADIUS} fill='none' strokeWidth='3.5' className='stroke-muted' />
        <circle
          cx='18'
          cy='18'
          r={RADIUS}
          fill='none'
          strokeWidth='3.5'
          strokeLinecap={percent > 0 ? "round" : "butt"}
          strokeDasharray={`${percent} ${100 - percent}`}
          className={cn("stroke-primary", percent === 0 && "hidden")}
        />
      </svg>
      <span className='absolute inset-0 flex items-center justify-center font-medium text-card-foreground text-xs tabular-nums'>
        {done}/{total}
      </span>
    </div>
  );
}
