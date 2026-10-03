import { CheckCircle2, ChevronRight, Hammer, Loader2, XCircle } from "lucide-react";

import type { BuilderDelegationItem, SelectableItem } from "@/hooks/analyticsSteps";
import { cn } from "@/libs/shadcn/utils";

interface BuilderDelegationCardProps {
  item: BuilderDelegationItem;
  onSelect: (item: SelectableItem) => void;
  /** Whether the delegation's panel is open; undefined when the host tracks no selection. */
  pressed?: boolean;
}

export default function BuilderDelegationCard({
  item,
  onSelect,
  pressed
}: BuilderDelegationCardProps) {
  const isRunning = item.status === "running";
  const isDone = item.status === "done";

  return (
    <div
      data-testid='builder-delegation-card'
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
          ) : isDone ? (
            <CheckCircle2 className='h-4 w-4 text-emerald-500' />
          ) : (
            <XCircle className='h-4 w-4 text-destructive' />
          )}
          <Hammer className='h-4 w-4 text-muted-foreground' />
          <span className='font-medium'>Builder Agent</span>
        </div>
        <span className='text-muted-foreground text-xs capitalize'>{item.status}</span>
      </div>

      {/* Request description */}
      <p className='line-clamp-2 text-muted-foreground text-xs'>{item.request}</p>

      {/* Error message */}
      {item.error && <p className='text-destructive text-xs'>{item.error}</p>}

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
