import { Pause, Play, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { cn } from "@/libs/utils/cn";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";

/**
 * Topbar "live" affordance — a colored heartbeat dot, a relative
 * "Updated N seconds ago" string, and a pause/resume + manual-refresh
 * button. Tells the operator the page is alive without leaving them
 * to wonder whether the numbers froze.
 *
 * Drops out of "live" tone when paused so the dot reads gray instead
 * of green. The pulsing animation is the cheap visual signal that the
 * loop is doing its thing.
 */
export const LiveIndicator = ({
  updatedAt,
  paused,
  onTogglePaused,
  onRefresh,
  isFetching
}: {
  /** Epoch ms of the most recent successful fetch, or undefined while loading. */
  updatedAt: number | undefined;
  paused: boolean;
  onTogglePaused: () => void;
  onRefresh: () => void;
  isFetching: boolean;
}) => {
  // Force a re-render every second so "5s ago" → "6s ago" without
  // re-fetching. Cheap; we already re-render every 5s anyway.
  const [, force] = useState(0);
  useEffect(() => {
    const id = window.setInterval(() => force((n) => n + 1), 1_000);
    return () => window.clearInterval(id);
  }, []);

  const ageSecs = updatedAt ? Math.max(0, Math.floor((Date.now() - updatedAt) / 1_000)) : null;
  const ageLabel = ageSecs === null ? "Loading…" : ageSecs < 1 ? "just now" : `${ageSecs}s ago`;

  return (
    <div className='flex items-center gap-3'>
      <div className='flex items-center gap-2 rounded-full border border-border/60 bg-card px-3 py-1.5'>
        <span className='relative flex size-2'>
          <span
            className={cn(
              "absolute inset-0 inline-flex size-full rounded-full",
              paused ? "bg-muted-foreground/40" : "animate-ping bg-success/60"
            )}
            aria-hidden
          />
          <span
            className={cn(
              "relative inline-flex size-2 rounded-full",
              paused ? ADMIN_TONE.muted.dot : ADMIN_TONE.ok.dot
            )}
            aria-hidden
          />
        </span>
        <span className='font-medium text-[11px] text-muted-foreground tabular-nums'>
          {paused ? "Paused" : ageLabel}
        </span>
      </div>
      <Button variant='outline' size='sm' onClick={onTogglePaused} className='h-8 gap-1.5 px-2.5'>
        {paused ? <Play className='size-3.5' /> : <Pause className='size-3.5' />}
        <span className='hidden text-xs sm:inline'>{paused ? "Resume" : "Pause"}</span>
      </Button>
      <Button
        variant='outline'
        size='sm'
        onClick={onRefresh}
        disabled={isFetching}
        className='h-8 gap-1.5 px-2.5'
      >
        <RefreshCw className={cn("size-3.5", isFetching && "animate-spin")} />
        <span className='hidden text-xs sm:inline'>Refresh</span>
      </Button>
    </div>
  );
};
