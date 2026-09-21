import { Activity, Cpu } from "lucide-react";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useWorkers } from "@/hooks/api/internalJobs";
import { cn } from "@/libs/utils/cn";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import {
  WORKER_LIVENESS_LABEL,
  WORKER_LIVENESS_TONE,
  workerLiveness
} from "@/pages/admin/components/workerLiveness";
import type { WorkerDto } from "@/services/api/internalJobs";
import { relativeTime } from "../../utils";
import { useLive } from "../LiveContext";

/**
 * Worker fleet — one card per `worker_id` seen in the last 24h. Liveness
 * is derived from how recent the last claim was: `ok` if < 60s, `warn`
 * if < 5min, `danger` after. This is the "Busy" tab from Sidekiq's
 * Web UI: at a glance the operator knows which workers are drawing
 * tasks right now and which are stale.
 *
 * Inflight count is rendered as a bar so a "busy worker" reads
 * visually, not just as a number. Layout is a horizontal-scrolling
 * rail when there are many workers — typical fleets have 2-8 so the
 * default grid is fine.
 */
export const WorkerFleetPanel = () => {
  const { paused } = useLive();
  const workers = useWorkers({ paused });

  return (
    <section className='space-y-3' data-testid='admin-internal-jobs-worker-fleet'>
      <header className='flex items-baseline justify-between'>
        <h3 className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
          Worker fleet
        </h3>
        {workers.data?.workers ? (
          <span className='font-medium text-[11px] text-muted-foreground tabular-nums'>
            {workers.data.workers.length} {workers.data.workers.length === 1 ? "worker" : "workers"}
          </span>
        ) : null}
      </header>

      <AdminAsync
        query={workers}
        noun='the worker fleet'
        skeleton={<Skeleton className='h-24 w-full' />}
        isEmpty={(d) => d.supported && d.workers.length === 0}
        empty={
          <AdminEmptyState
            icon={Cpu}
            title='No worker activity in the last 24h.'
            description='Either the queue has been idle, or no `oxy worker` process is connected.'
          />
        }
      >
        {(d) =>
          !d.supported ? (
            <AdminEmptyState
              icon={Cpu}
              title="Worker fleet info isn't available for this deployment."
              description='The backing column is not present on this schema.'
            />
          ) : (
            <WorkerGrid workers={d.workers} />
          )
        }
      </AdminAsync>
    </section>
  );
};

const WorkerGrid = ({ workers }: { workers: WorkerDto[] }) => {
  // Hoisted out of the worker .map so the busiest-worker scan is O(n)
  // instead of O(n²). Trivial at typical fleet sizes (2-8) but cheap.
  const inflightMax = Math.max(...workers.map((w) => w.inflight_count), 1);

  return (
    <div className='grid grid-cols-1 gap-2 md:grid-cols-2 xl:grid-cols-3'>
      {workers.map((w) => {
        const liveness = workerLiveness(w.last_claim_at);
        const label = WORKER_LIVENESS_LABEL[liveness];
        const tone = WORKER_LIVENESS_TONE[liveness];
        const v = ADMIN_TONE[tone];
        const inflightPct = (w.inflight_count / inflightMax) * 100;
        return (
          <div
            key={w.worker_id}
            className='flex flex-col gap-2 rounded-lg border border-border/60 bg-card p-3'
            data-testid={`admin-internal-jobs-worker-${w.worker_id}`}
          >
            <div className='flex items-center justify-between gap-2'>
              <div className='flex min-w-0 items-center gap-2'>
                <span className={cn("size-2 shrink-0 rounded-full", v.dot)} aria-hidden />
                <span className='truncate font-mono text-[11px]'>{w.worker_id}</span>
              </div>
              <span className={cn("font-medium text-[10px] uppercase tracking-wide", v.text)}>
                {label}
              </span>
            </div>
            <div className='flex items-center gap-2 text-[11px] text-muted-foreground tabular-nums'>
              <Activity className='size-3' />
              Last claim {w.last_claim_at ? relativeTime(w.last_claim_at) : "—"}
            </div>
            <div className='space-y-1'>
              <div className='flex items-center justify-between text-[10px] text-muted-foreground'>
                <span className='inline-flex items-center gap-1 uppercase tracking-wide'>
                  <Cpu className='size-3' />
                  Inflight
                </span>
                <span className='font-medium text-foreground tabular-nums'>{w.inflight_count}</span>
              </div>
              <div className='h-1 w-full overflow-hidden rounded-full bg-muted'>
                <div
                  className={cn("h-full rounded-full transition-[width]", v.dot)}
                  style={{ width: `${inflightPct}%` }}
                />
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
};

/** Liveness from how recent the last claim was, as a label plus one of the
 *  console's five tones — previously four bespoke palette triples. */
