import { AlertTriangle, CheckCircle, Clock, Inbox, RefreshCw, Skull, XCircle } from "lucide-react";
import type React from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useQueueHealth from "@/hooks/api/coordinator/useQueueHealth";
import { cn } from "@/libs/shadcn/utils";
import type { QueueTaskEntry } from "@/services/api/coordinator";

// ── Stat card ───────────────────────────────────────────────────────────────

const StatCard: React.FC<{
  label: string;
  value: number;
  icon: React.ElementType;
  className?: string;
}> = ({ label, value, icon: Icon, className }) => (
  <div className='flex items-center gap-3 rounded-lg border border-border bg-card px-4 py-3'>
    <Icon className={cn("h-5 w-5", className)} />
    <div>
      <p className='font-semibold text-lg'>{value}</p>
      <p className='text-muted-foreground text-xs'>{label}</p>
    </div>
  </div>
);

// ── Task row ────────────────────────────────────────────────────────────────

const TaskRow: React.FC<{ task: QueueTaskEntry }> = ({ task }) => {
  const heartbeatAge = task.last_heartbeat
    ? `${Math.floor((Date.now() - new Date(task.last_heartbeat).getTime()) / 1000)}s ago`
    : "never";

  return (
    <div className='flex items-center gap-4 border-border border-b px-4 py-2 last:border-b-0'>
      <span
        className={cn(
          "font-medium text-xs",
          task.queue_status === "dead" ? "text-destructive" : "text-warning"
        )}
      >
        {task.queue_status}
      </span>
      <div className='min-w-0 flex-1'>
        <span className='font-mono text-xs'>{task.task_id.slice(0, 12)}</span>
        <span className='mx-2 text-muted-foreground text-xs'>run: {task.run_id.slice(0, 8)}</span>
        {task.worker_id && (
          <span className='text-muted-foreground text-xs'>worker: {task.worker_id}</span>
        )}
      </div>
      <div className='shrink-0 text-right'>
        <span className='text-muted-foreground text-xs'>
          claims: {task.claim_count}/{task.max_claims}
        </span>
      </div>
      <div className='w-24 shrink-0 text-right'>
        <span className='text-muted-foreground text-xs'>heartbeat: {heartbeatAge}</span>
      </div>
    </div>
  );
};

// ── Page ────────────────────────────────────────────────────────────────────

const QueueHealthPage: React.FC = () => {
  const { data, isPending, error, refetch } = useQueueHealth();

  if (isPending) {
    return (
      <div className='flex h-full items-center justify-center'>
        <Spinner className='h-6 w-6' />
      </div>
    );
  }

  if (error) {
    return (
      <div className='flex h-full flex-col items-center justify-center gap-2'>
        <p className='text-destructive text-sm'>Failed to load queue health</p>
        <Button variant='outline' size='sm' onClick={() => refetch()}>
          Retry
        </Button>
      </div>
    );
  }

  if (!data) return null;

  const totalProcessed = data.completed + data.failed + data.cancelled;

  return (
    <div className='flex h-full flex-col'>
      {/* Header */}
      <div className='flex items-center justify-between border-border border-b px-4 py-3'>
        <div>
          <h2 className='font-semibold text-base'>Queue Health</h2>
          <p className='text-muted-foreground text-xs'>
            {totalProcessed} processed, {data.queued + data.claimed} in-flight
          </p>
        </div>
        <Button variant='ghost' size='icon' onClick={() => refetch()} className='h-8 w-8'>
          <RefreshCw className='h-4 w-4' />
        </Button>
      </div>

      <div className='flex-1 overflow-y-auto'>
        {/* Stats grid */}
        <div className='grid grid-cols-3 gap-3 p-4'>
          <StatCard label='Queued' value={data.queued} icon={Inbox} className='text-primary' />
          <StatCard label='Claimed' value={data.claimed} icon={Clock} className='text-warning' />
          <StatCard
            label='Completed'
            value={data.completed}
            icon={CheckCircle}
            className='text-emerald-500'
          />
          <StatCard
            label='Failed'
            value={data.failed}
            icon={XCircle}
            className='text-destructive'
          />
          <StatCard
            label='Cancelled'
            value={data.cancelled}
            icon={AlertTriangle}
            className='text-muted-foreground'
          />
          <StatCard
            label='Dead-lettered'
            value={data.dead}
            icon={Skull}
            className='text-destructive'
          />
        </div>

        {/* Stale tasks */}
        <div className='px-4 pb-2'>
          <h3 className='mb-2 font-medium text-sm'>Stale Tasks ({data.stale_tasks.length})</h3>
          <div className='rounded-lg border border-border'>
            {data.stale_tasks.length === 0 ? (
              <p className='px-4 py-4 text-center text-muted-foreground text-sm'>
                No stale tasks — all heartbeats healthy
              </p>
            ) : (
              data.stale_tasks.map((t) => <TaskRow key={t.task_id} task={t} />)
            )}
          </div>
        </div>

        {/* Dead-lettered tasks */}
        <div className='px-4 pt-4 pb-4'>
          <h3 className='mb-2 font-medium text-sm'>
            Dead-lettered Tasks (
            {data.dead_tasks_truncated
              ? `newest ${data.dead_tasks.length} of ${data.dead}`
              : data.dead_tasks.length}
            )
          </h3>
          <div className='rounded-lg border border-border'>
            {data.dead_tasks.length === 0 ? (
              <p className='px-4 py-4 text-center text-muted-foreground text-sm'>
                No dead-lettered tasks
              </p>
            ) : (
              data.dead_tasks.map((t) => <TaskRow key={t.task_id} task={t} />)
            )}
          </div>
        </div>
      </div>
    </div>
  );
};

export default QueueHealthPage;
