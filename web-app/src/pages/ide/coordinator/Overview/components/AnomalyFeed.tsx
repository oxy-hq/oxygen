import { ChevronRight, Sparkles, TrendingUp } from "lucide-react";
import type React from "react";
import { useMemo } from "react";
import { useNavigate } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import { JobTypeBadge } from "../../components/JobTypeBadge";
import type { NormalizedRun } from "../../components/runModel";
import { StatusBadge } from "../../components/StatusBadge";
import { SystemBadge } from "../../components/SystemBadge";
import { useCoordinatorRoutes } from "../../components/useCoordinatorRoutes";
import { formatRelative } from "../../components/utils";

type FeedItemKind = "failure" | "anomaly";

interface FeedItem {
  kind: FeedItemKind;
  run: NormalizedRun;
}

/**
 * Recent failures & anomalies. Hard failures (status="failed") and slow-run
 * anomalies (duration > 2× per-job p50) ride the same feed sorted by recency
 * — a green-but-slow run reads as urgently as a red one. Cost-spike and
 * row-drop anomalies still need backend instrumentation per job type.
 */
export const AnomalyFeed: React.FC<{ runs: NormalizedRun[] }> = ({ runs }) => {
  const navigate = useNavigate();
  const routes = useCoordinatorRoutes();

  const items = useMemo<FeedItem[]>(() => {
    const out: FeedItem[] = [];
    for (const r of runs) {
      if (r.status === "failed") out.push({ kind: "failure", run: r });
      else if (r.anomaly) out.push({ kind: "anomaly", run: r });
    }
    return out
      .sort((a, b) => new Date(b.run.startedAt).getTime() - new Date(a.run.startedAt).getTime())
      .slice(0, 10);
  }, [runs]);

  return (
    <div className='border border-border bg-card'>
      <div className='border-border border-b px-3 py-2'>
        <h3 className='font-semibold text-sm'>Recent failures &amp; anomalies</h3>
      </div>
      {items.length === 0 ? (
        <p className='px-3 py-6 text-center text-muted-foreground text-sm'>
          No failures or anomalies in this window.
        </p>
      ) : (
        <ul>
          {items.map(({ kind, run }) => (
            <li key={`${kind}-${run.runId}`}>
              <button
                type='button'
                data-testid='coordinator-anomaly-item'
                onClick={() => navigate(routes.RUN_DETAIL(run.runId))}
                className='flex w-full items-center gap-3 border-border border-b px-3 py-2 text-left last:border-b-0 hover:bg-muted/50'
              >
                {kind === "failure" ? (
                  <StatusBadge status={run.status} iconOnly />
                ) : (
                  <TrendingUp
                    className={cn(
                      "h-3.5 w-3.5 shrink-0",
                      run.anomaly?.severity === "critical" ? "text-destructive" : "text-warning"
                    )}
                  />
                )}
                {run.isSystem ? (
                  <SystemBadge variant='icon' />
                ) : (
                  <JobTypeBadge type={run.jobType} variant='icon' />
                )}
                <span className='min-w-0 flex-1 truncate text-sm'>{run.title}</span>
                {kind === "failure" && run.errorMessage && (
                  <span className='hidden max-w-xs truncate text-muted-foreground text-xs md:block'>
                    {run.errorMessage}
                  </span>
                )}
                {kind === "anomaly" && run.anomaly && (
                  <span
                    className={cn(
                      "shrink-0 px-1.5 py-0.5 font-medium text-xs",
                      run.anomaly.severity === "critical"
                        ? "bg-destructive/10 text-destructive"
                        : "bg-warning/10 text-warning"
                    )}
                  >
                    {run.anomaly.detail}
                  </span>
                )}
                <span className='shrink-0 text-muted-foreground text-xs tabular-nums'>
                  {formatRelative(run.startedAt)}
                </span>
                <ChevronRight className='h-4 w-4 shrink-0 text-muted-foreground' />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className='flex items-start gap-1.5 border-border border-t px-3 py-2 text-muted-foreground text-xs'>
        <Sparkles className='mt-0.5 h-3.5 w-3.5 shrink-0' />
        <span>
          Duration anomalies are live (per-job p50 baseline). Cost-spike and row-drop detection ship
          with per-type metrics instrumentation.
        </span>
      </div>
    </div>
  );
};
