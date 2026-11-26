import { AlertCircle, CheckCircle2, Clock, Coins, Timer } from "lucide-react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { cn } from "@/libs/shadcn/utils";
import type { Trace } from "@/services/api/traces";
import { formatDuration, formatTimeAgo, SpanIcon } from "../../utils";
import { deriveTraceRow } from "./traceRow";

interface TraceCardProps {
  trace: Trace;
  onClick: () => void;
  selected?: boolean;
  /** Selection cap reached — an unselected card can no longer be checked. */
  selectDisabled?: boolean;
  onToggleSelect?: () => void;
}

export function TraceCard({
  trace,
  onClick,
  selected = false,
  selectDisabled = false,
  onToggleSelect
}: TraceCardProps) {
  const row = deriveTraceRow(trace);

  return (
    <div
      className={cn(
        "flex cursor-pointer items-start gap-2 border px-3 py-2 transition-colors hover:bg-accent",
        selected && "border-primary bg-accent"
      )}
      onClick={onClick}
    >
      {onToggleSelect && (
        <Checkbox
          checked={selected}
          disabled={!selected && selectDisabled}
          onClick={(e) => e.stopPropagation()}
          onCheckedChange={() => onToggleSelect()}
          aria-label='Select trace to compare'
          className='mt-0.5'
        />
      )}
      <div className='flex min-w-0 flex-1 flex-col gap-1'>
        <div className='flex items-center gap-2'>
          {row.isError ? (
            <AlertCircle className='h-4 w-4 flex-shrink-0 text-destructive' />
          ) : (
            <CheckCircle2 className='h-4 w-4 flex-shrink-0 text-success' />
          )}
          <SpanIcon
            spanName={trace.spanName}
            className='h-4 w-4 flex-shrink-0 text-muted-foreground'
          />
          <span className='flex-1 truncate font-medium text-sm'>{row.title}</span>
          <span className='flex flex-shrink-0 items-center gap-1 text-muted-foreground text-xs'>
            <Clock className='h-3 w-3' />
            {formatTimeAgo(row.timestamp)}
          </span>
        </div>
        <div className='ml-6 flex items-center gap-2'>
          <Badge variant='outline' className='text-xs'>
            {row.spanLabel}
          </Badge>

          {row.entityRef && <span className='text-muted-foreground text-xs'>{row.entityRef}</span>}

          <Badge variant='secondary' className='gap-1 text-xs'>
            <Timer className='h-3 w-3' />
            {formatDuration(row.durationMs)}
          </Badge>

          {!!row.tokensTotal && row.tokensTotal !== 0 && (
            <Badge variant='outline' className='gap-1 text-xs'>
              <Coins className='h-3 w-3' />
              {row.tokensTotal.toLocaleString()}
            </Badge>
          )}
        </div>
      </div>
    </div>
  );
}
