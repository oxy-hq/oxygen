import { RefreshCw, Unplug } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";

interface IdeUnavailablePanelProps {
  /** Headline; defaults to the generic developer-environment message. */
  title?: string;
  /** One line on what's paused and that it resumes — keep it calm. */
  description?: string;
  /** Refetch the failing query. Omit to hide the Retry button. */
  onRetry?: () => void;
  /** Show the spinner while a retry is in flight. */
  retrying?: boolean;
  className?: string;
}

/**
 * Inline placeholder for a surface that needs the developer-environment
 * singleton (the working copy / local DuckDB execution env) when it is
 * temporarily unreachable. The calm, non-blocking sibling of the app-wide
 * `IdeUnavailableBanner`: shown in PLACE of a generic error so an ide restart
 * reads as "paused, resuming" rather than "broken". Detect the condition with
 * `isIdeUnavailableError` from `libs/utils/ideHealth`.
 */
export default function IdeUnavailablePanel({
  title = "Oxygen Nexus is temporarily unavailable",
  description = "This view needs Oxygen Nexus, which is restarting. It will resume shortly.",
  onRetry,
  retrying = false,
  className
}: IdeUnavailablePanelProps) {
  return (
    <div
      role='status'
      aria-live='polite'
      data-testid='ide-unavailable-panel'
      className={cn(
        "flex min-h-60 flex-col items-center justify-center gap-3 rounded-lg border border-warning/30 bg-card/60 p-6 text-center",
        className
      )}
    >
      <span className='relative flex size-10 items-center justify-center rounded-md bg-warning/10 text-warning'>
        <Unplug className='size-5' />
        <span className='absolute -top-0.5 -right-0.5 size-2 animate-pulse rounded-full bg-warning' />
      </span>

      <div className='max-w-sm space-y-1'>
        <p className='font-medium text-foreground text-sm'>{title}</p>
        <p className='text-muted-foreground text-xs'>{description}</p>
      </div>

      {onRetry && (
        <button
          type='button'
          onClick={onRetry}
          disabled={retrying}
          className={cn(
            "inline-flex items-center gap-1.5 rounded-md border border-warning/40 bg-warning/10 px-2.5 py-1",
            "font-medium text-foreground text-xs transition-colors hover:bg-warning/20 disabled:opacity-60"
          )}
        >
          <RefreshCw className={cn("size-3.5", retrying && "animate-spin")} />
          {retrying ? "Retrying…" : "Retry"}
        </button>
      )}
    </div>
  );
}
