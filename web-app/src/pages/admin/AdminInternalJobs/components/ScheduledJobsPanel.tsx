import { Clock, Loader2, Play } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useRunScheduledJob, useScheduledJobs } from "@/hooks/api/internalJobs";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { relativeTime } from "../../utils";
import { formatInterval } from "../utils";

/**
 * Periodic system loops registered with the runtime (reaper, retention prune,
 * health probe, etc.). Each row carries a "Run now" button when the backend
 * exposes a manual-trigger path for the job. The classic Hangfire pattern:
 * don't make the operator wait for a tick if they need it right now.
 */
export const ScheduledJobsPanel = () => {
  const jobs = useScheduledJobs();
  const runJob = useRunScheduledJob();

  return (
    <section className='space-y-3' data-testid='admin-internal-jobs-scheduled'>
      <header>
        <h3 className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
          Scheduled jobs
        </h3>
      </header>

      <AdminAsync
        query={jobs}
        noun='scheduled jobs'
        skeleton={<Skeleton className='h-24 w-full' />}
      >
        {(data) => (
          <div className='divide-y divide-border/60 overflow-hidden rounded-lg border border-border/60 bg-card'>
            {data.map((job) => {
              const triggerPath = job.trigger_path;
              const running = runJob.isPending && runJob.variables?.name === job.name;
              return (
                <div
                  key={job.name}
                  className='flex flex-col gap-3 p-3 sm:flex-row sm:items-center sm:justify-between'
                  data-testid={`admin-internal-jobs-scheduled-row-${job.name}`}
                >
                  <div className='flex min-w-0 flex-col gap-1'>
                    <div className='flex items-center gap-2'>
                      <span className='font-mono text-xs'>{job.name}</span>
                      <span className='inline-flex items-center gap-1 rounded-full bg-muted/60 px-1.5 py-0.5 font-medium text-[10px] text-muted-foreground tabular-nums'>
                        <Clock className='size-2.5' />
                        every {formatInterval(job.interval_secs)}
                      </span>
                    </div>
                    <p className='text-muted-foreground text-xs'>{job.description}</p>
                    <p className='text-[11px] text-muted-foreground tabular-nums'>
                      Last run: {job.last_known_run_at ? relativeTime(job.last_known_run_at) : "—"}
                    </p>
                  </div>
                  {triggerPath ? (
                    <Button
                      size='sm'
                      variant='outline'
                      disabled={running}
                      onClick={() => runJob.mutate({ name: job.name, triggerPath })}
                      className='shrink-0 gap-1.5'
                    >
                      {running ? (
                        <Loader2 className='size-3.5 animate-spin' />
                      ) : (
                        <Play className='size-3.5' />
                      )}
                      Run now
                    </Button>
                  ) : null}
                </div>
              );
            })}
          </div>
        )}
      </AdminAsync>
    </section>
  );
};
