import { Info } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/libs/shadcn/utils";
import { type Tone, ToneIcon } from "@/pages/ide/Experiments/components/glance/tone";
import { Hinted } from "./Hinted";

/** One figure under a plain label, optionally coloured and explained on hover. */
export function StatTile({
  id,
  label,
  value,
  sub,
  tone,
  tag,
  testId,
  hint
}: {
  id: string;
  label: string;
  value: ReactNode;
  sub?: ReactNode;
  tone?: Tone;
  /** A small label beside the tile's name, e.g. "Assumption". */
  tag?: ReactNode;
  testId?: string;
  /** What the figure means, shown when the tile is hovered or focused. */
  hint?: string;
}) {
  const body = (
    <div
      className={cn("min-w-0 bg-card p-3", hint && "cursor-help hover:bg-muted/40")}
      data-testid={testId ?? `experiment-stat-${id}`}
      data-tone={tone}
      // biome-ignore lint/a11y/noNoninteractiveTabindex: focus opens the explanation for keyboard users
      tabIndex={hint ? 0 : undefined}
    >
      <div className='flex items-center gap-1.5 text-muted-foreground text-xs'>
        {label}
        {tag}
        {hint && <Info aria-hidden className='ml-auto size-3 opacity-60' />}
      </div>
      <div className='mt-1 flex items-center gap-1.5 font-semibold text-lg tabular-nums'>
        {tone && <ToneIcon tone={tone} className='size-4' />}
        <span className='truncate'>{value}</span>
      </div>
      {sub && <div className='mt-0.5 text-muted-foreground text-xs'>{sub}</div>}
    </div>
  );
  return hint ? <Hinted hint={hint}>{body}</Hinted> : body;
}

/**
 * A grid of stat tiles with hairline dividers. Each tile draws its own
 * outline (a 1px ring into the gaps around it, clipped at the
 * frame), so a short last row leaves plain card instead of a gray block.
 */
export function StatGrid({
  children,
  testId,
  className
}: {
  children: ReactNode;
  testId?: string;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "grid grid-cols-2 gap-px overflow-hidden rounded-md border bg-card *:shadow-[0_0_0_1px_var(--border)] sm:grid-cols-3",
        className
      )}
      data-testid={testId}
    >
      {children}
    </div>
  );
}
