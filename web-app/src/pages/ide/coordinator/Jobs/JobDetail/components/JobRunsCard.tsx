import { ChevronRight, History } from "lucide-react";
import type React from "react";
import { useMemo } from "react";
import { useNavigate } from "react-router-dom";
import useActiveRuns from "@/hooks/api/coordinator/useActiveRuns";
import useRunHistory from "@/hooks/api/coordinator/useRunHistory";
import { cn } from "@/libs/shadcn/utils";
import { Elapsed } from "../../../components/Elapsed";
import { ErrorState, LoadingState } from "../../../components/PageState";
import { mergeRuns } from "../../../components/runModel";
import { StatusBadge } from "../../../components/StatusBadge";
import { useCoordinatorRoutes } from "../../../components/useCoordinatorRoutes";
import { formatTimestamp, shortId } from "../../../components/utils";

const PAGE = 25;

/**
 * Per-job run history. Active runs come from the global SSE-backed query
 * (so a freshly-fired run shows up immediately) and are merged with the
 * paged history; both are filtered down to runs whose `schedule_id` matches
 * this job — possible only after the runtime schedule_id linkage shipped.
 */
export const JobRunsCard: React.FC<{ scheduleId: string }> = ({ scheduleId }) => {
  const navigate = useNavigate();
  const routes = useCoordinatorRoutes();
  const history = useRunHistory({ limit: PAGE, offset: 0, schedule_id: scheduleId });
  const active = useActiveRuns();

  const runs = useMemo(() => {
    const activeForJob = (active.data?.runs ?? []).filter((r) => r.schedule_id === scheduleId);
    return mergeRuns(activeForJob, history.data?.runs ?? []);
  }, [active.data, history.data, scheduleId]);

  return (
    <div className='border border-border bg-card lg:col-span-2'>
      <div className='flex items-center justify-between border-border border-b px-3 py-2'>
        <div className='flex items-center gap-2'>
          <History className='h-4 w-4 text-muted-foreground' />
          <h3 className='font-semibold text-sm'>Recent runs</h3>
          <span className='text-muted-foreground text-xs'>{history.data?.total ?? 0} total</span>
        </div>
      </div>
      {history.isPending ? (
        <div className='py-8'>
          <LoadingState />
        </div>
      ) : history.error ? (
        <ErrorState message='Failed to load runs' onRetry={() => history.refetch()} />
      ) : runs.length === 0 ? (
        <p className='px-3 py-6 text-center text-muted-foreground text-sm'>
          No runs yet for this job — it'll show up here after the first fire.
        </p>
      ) : (
        <ul>
          {runs.map((run) => (
            <li key={run.runId}>
              <button
                type='button'
                onClick={() => navigate(routes.RUN_DETAIL(run.runId))}
                className={cn(
                  "flex w-full items-center gap-3 border-border border-b px-3 py-2 text-left",
                  "last:border-b-0 hover:bg-muted/50"
                )}
              >
                <StatusBadge status={run.status} />
                <span className='min-w-0 flex-1 truncate text-sm'>{run.title}</span>
                {run.attempt > 0 && (
                  <span className='shrink-0 text-warning text-xs'>attempt {run.attempt + 1}</span>
                )}
                <span className='hidden shrink-0 text-muted-foreground text-xs md:inline'>
                  {formatTimestamp(run.startedAt)}
                </span>
                <span className='shrink-0 text-muted-foreground text-xs tabular-nums'>
                  <Elapsed
                    startIso={run.startedAt}
                    endIso={run.endedAt ?? undefined}
                    live={run.live}
                  />
                </span>
                <span className='hidden shrink-0 font-mono text-muted-foreground text-xs sm:inline'>
                  {shortId(run.runId)}
                </span>
                <ChevronRight className='h-4 w-4 shrink-0 text-muted-foreground' />
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
};
