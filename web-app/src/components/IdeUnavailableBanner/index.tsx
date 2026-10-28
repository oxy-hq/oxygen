import { useQueryClient } from "@tanstack/react-query";
import { RefreshCw, Unplug, X } from "lucide-react";
import { useState } from "react";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import useFactoryAccess from "@/hooks/useFactoryAccess";
import useIdeHealth from "@/hooks/useIdeHealth";
import { cn } from "@/libs/shadcn/utils";
import { dismissIdeUnavailableBanner } from "@/libs/utils/ideHealth";

/**
 * App-wide notice shown when Oxygen Factory (the developer environment — file
 * editing, Git, the IDE) can't be reached. Deliberately non-blocking and calm:
 * the data plane — agents, apps, dashboards — keeps working, so this is a
 * warning, not an error. Mounted once in `OrgGuard`; driven by the Axios
 * interceptor via `ideHealth`.
 */
export default function IdeUnavailableBanner() {
  const { unavailable, dismissed, lastPath, since } = useIdeHealth();
  const queryClient = useQueryClient();
  const [retrying, setRetrying] = useState(false);
  // Apps-first: someone who may not use the Factory is not told about it, its
  // files or git, nor shown the API path that failed.
  const appsFirst = useAppsFirstExperience();
  const factoryUser = useFactoryAccess();
  const plain = appsFirst && !factoryUser;

  if (!unavailable || dismissed) return null;

  const onRetry = () => {
    setRetrying(true);
    // Refetch in-flight data; a response served by the IDE backend clears this
    // banner through the interceptor. Leave the spinner up briefly either way.
    void queryClient.invalidateQueries();
    window.setTimeout(() => setRetrying(false), 800);
  };

  const startedAt =
    since != null
      ? new Date(since).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
      : null;

  return (
    <div
      role='status'
      aria-live='polite'
      data-testid='ide-unavailable-banner'
      className={cn(
        "fixed right-4 bottom-4 left-4 z-50 sm:left-auto sm:w-96",
        "rounded-lg border border-warning/30 bg-card/95 shadow-lg backdrop-blur",
        "fade-in slide-in-from-bottom-2 animate-in duration-300"
      )}
    >
      <div className='flex gap-3 p-3.5'>
        <span className='relative mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-md bg-warning/10 text-warning'>
          <Unplug className='size-4' />
          <span className='absolute -top-0.5 -right-0.5 size-2 animate-pulse rounded-full bg-warning' />
        </span>

        <div className='min-w-0 flex-1'>
          <p className='font-medium text-foreground text-sm'>
            {plain ? "Some live data is paused" : "Oxygen Factory is temporarily unavailable"}
          </p>
          <p className='mt-0.5 text-muted-foreground text-xs'>
            {plain
              ? "Live app runs are paused for a moment. Chat and your saved data keep working."
              : "File editing, Git, and live app runs are paused right now. Chat and your saved data keep working."}
          </p>

          {lastPath && !plain && (
            <p
              className='mt-2 truncate font-mono text-muted-foreground/70 text-xs'
              title={lastPath}
            >
              {lastPath}
              {startedAt && <span className='text-muted-foreground/50'> · since {startedAt}</span>}
            </p>
          )}

          <div className='mt-2.5'>
            <button
              type='button'
              onClick={onRetry}
              disabled={retrying}
              className={cn(
                "inline-flex items-center gap-1.5 rounded-md border border-warning/40 bg-warning/10 px-2.5 py-1",
                "font-medium text-foreground text-xs transition-colors hover:bg-warning/20",
                "disabled:opacity-60"
              )}
            >
              <RefreshCw className={cn("size-3.5", retrying && "animate-spin")} />
              {retrying ? "Retrying…" : "Retry"}
            </button>
          </div>
        </div>

        <button
          type='button'
          aria-label='Dismiss'
          onClick={dismissIdeUnavailableBanner}
          className='-mt-1 -mr-1 inline-flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground/60 transition-colors hover:bg-muted hover:text-foreground'
        >
          <X className='size-3.5' />
        </button>
      </div>
    </div>
  );
}
