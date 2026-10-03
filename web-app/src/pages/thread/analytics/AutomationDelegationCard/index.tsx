import { CheckCircle2, ChevronRight, Loader2, XCircle } from "lucide-react";

import type { AutomationItem, SelectableItem } from "@/hooks/analyticsSteps";
import { cn } from "@/libs/shadcn/utils";

interface AutomationDelegationCardProps {
  item: AutomationItem;
  onSelect: (item: SelectableItem) => void;
  /** Whether the run's panel is open; undefined when the host tracks no selection. */
  pressed?: boolean;
}

export default function AutomationDelegationCard({
  item,
  onSelect,
  pressed
}: AutomationDelegationCardProps) {
  const total = item.steps.length;
  const done = item.stepsDone;
  const isRunning = item.isStreaming;
  const progressPct = total > 0 ? (done / total) * 100 : 0;

  return (
    <div
      data-testid='automation-delegation-card'
      className={cn(
        "space-y-2 rounded-lg border p-3 text-sm",
        isRunning ? "border-primary/30 bg-primary/5" : "border-border bg-muted/30"
      )}
    >
      {/* Header */}
      <div className='flex items-center justify-between'>
        <div className='flex items-center gap-2'>
          {isRunning ? (
            <Loader2 className='h-4 w-4 animate-spin text-primary' />
          ) : done >= total ? (
            <CheckCircle2 className='h-4 w-4 text-emerald-500' />
          ) : (
            <XCircle className='h-4 w-4 text-destructive' />
          )}
          <span className='font-medium'>{item.automationName}</span>
        </div>
        <span className='text-muted-foreground text-xs'>
          {done}/{total} steps
        </span>
      </div>

      {/* Progress bar */}
      <div className='h-1.5 w-full rounded-full bg-muted' data-testid='progress-bar'>
        <div
          className='h-full rounded-full bg-primary transition-all duration-300'
          style={{ width: `${progressPct}%` }}
          data-testid='progress-fill'
        />
      </div>

      {/* View details button */}
      <button
        type='button'
        aria-pressed={pressed}
        onClick={() => onSelect(item)}
        className='flex items-center gap-1 text-primary text-xs hover:underline'
        data-testid='view-details-button'
      >
        View details
        <ChevronRight className='h-3 w-3' />
      </button>
    </div>
  );
}
