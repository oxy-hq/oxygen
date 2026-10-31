import type { CSSProperties } from "react";
import { cn } from "@/libs/shadcn/utils";

/** The ring's circle: a circumference of 100 makes the dash length the percentage. */
const RADIUS = 100 / (2 * Math.PI);

/**
 * "N of M" as a ring beside the headline, with the numbers in it. Every widget
 * card is one column wide, so a bar's own line would cost a row for less.
 */
export function WidgetProgress({
  done,
  total,
  testId
}: {
  done: number;
  total: number;
  testId: string;
}) {
  const percent = total > 0 ? Math.min(100, Math.max(0, (done / total) * 100)) : 0;
  return (
    <div
      role='progressbar'
      aria-valuemin={0}
      aria-valuemax={total}
      aria-valuenow={done}
      aria-label={`${done} of ${total}`}
      data-testid={testId}
      className='relative size-14 shrink-0'
    >
      <svg viewBox='0 0 36 36' className='size-full -rotate-90' aria-hidden>
        <circle
          cx='18'
          cy='18'
          r={RADIUS}
          fill='none'
          strokeWidth='3.5'
          className='stroke-border'
        />
        {/* Fills from empty to its value when it appears, then follows it (motion.css). */}
        <circle
          cx='18'
          cy='18'
          r={RADIUS}
          fill='none'
          strokeWidth='3.5'
          strokeLinecap={percent > 0 ? "round" : "butt"}
          style={{ "--ring-fill": percent } as CSSProperties}
          className={cn("motion-ring-arc stroke-primary", percent === 0 && "hidden")}
        />
      </svg>
      <span className='absolute inset-0 flex items-center justify-center font-medium text-card-foreground text-xs tabular-nums'>
        {done}/{total}
      </span>
    </div>
  );
}
