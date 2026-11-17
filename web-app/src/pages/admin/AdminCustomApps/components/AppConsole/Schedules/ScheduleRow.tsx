import { Loader2, Play } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { useRunFunctionAsSchedule } from "@/hooks/api/customApps/useAppInvocations";
import { errMessage } from "@/hooks/api/errMessage";
import { timeAgo } from "@/libs/utils/date";
import { describeCron } from "@/pages/ide/coordinator/components/utils";
import type { AppInvocation, CustomApp } from "@/types/apps";
import { InvocationRow } from "../../AppDetail/components/Functions/InvocationRow";
import { RunPanel } from "../../AppDetail/components/Functions/RunPanel";
import { formatInZone, isDue, type ScheduledFunction } from "./schedules";

/**
 * One scheduled function: its cron and zone, when it fires next, its last cron
 * fire (the line opens that invocation's waterfall), and Run now as the schedule.
 */
export const ScheduleRow = ({
  app,
  environment,
  schedule,
  fires,
  last,
  lastPending
}: {
  app: CustomApp;
  environment: string;
  schedule: ScheduledFunction;
  /** The environment fires its schedules: false when its switch is off. */
  fires: boolean;
  last: AppInvocation | null;
  lastPending: boolean;
}) => {
  const run = useRunFunctionAsSchedule();
  const [runId, setRunId] = useState<string | null>(null);
  const described = describeCron(schedule.cron);

  const runNow = () =>
    run.mutate(
      { id: app.id, name: schedule.name, environment },
      {
        onSuccess: ({ run_id }) => setRunId(run_id),
        onError: (e) => toast.error(errMessage(e, "Couldn't start the run"))
      }
    );

  return (
    <li
      className='flex flex-col gap-1 border-border/40 border-b py-2 last:border-b-0'
      data-testid={`admin-app-schedule-${schedule.name}`}
    >
      <div className='flex flex-wrap items-baseline gap-x-3 gap-y-1 text-xs'>
        <span className='font-medium font-mono'>{schedule.name}</span>
        <code className='font-mono text-muted-foreground' title={described}>
          {schedule.cron}
        </code>
        <span className='text-muted-foreground'>
          {described !== schedule.cron ? `${described} · ` : ""}
          {schedule.timezone}
          {schedule.timezoneDeclared ? "" : " (default)"}
        </span>
        <span className='ml-auto flex items-center gap-2'>
          <NextRun schedule={schedule} fires={fires} />
          <Button
            variant='outline'
            size='sm'
            className='h-6 gap-1 px-2 text-xs'
            disabled={run.isPending}
            onClick={runNow}
            title='Run once as the schedule would: no input, the system identity, this environment’s build — whether or not its switch is on.'
            data-testid='admin-app-schedule-run'
          >
            {run.isPending ? (
              <Loader2 className='size-3 animate-spin' />
            ) : (
              <Play className='size-3' />
            )}
            Run now
          </Button>
        </span>
      </div>
      {lastPending ? (
        <p className='pl-1 text-muted-foreground text-xs'>Reading the last fire…</p>
      ) : last ? (
        <div className='flex flex-col'>
          <span className='text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
            Last scheduled run
          </span>
          <ul>
            <InvocationRow appId={app.id} iv={last} appRef={app} />
          </ul>
        </div>
      ) : (
        <p className='text-muted-foreground text-xs' data-testid='admin-app-schedule-never'>
          No scheduled run recorded here.
        </p>
      )}
      {runId && <RunPanel key={runId} appId={app.id} runId={runId} />}
    </li>
  );
};

/** When it fires next, on the cron's own clock — or why it will not. */
const NextRun = ({ schedule, fires }: { schedule: ScheduledFunction; fires: boolean }) => {
  if (!schedule.nextRunAt) {
    return (
      <span className='text-muted-foreground' data-testid='admin-app-schedule-next'>
        no next run: the scheduler cannot read this cron
      </span>
    );
  }
  const at = formatInZone(schedule.nextRunAt, schedule.timezone);
  const local = new Date(schedule.nextRunAt).toLocaleString();
  // Past: the instant the server read has gone by, and the panel is about to read
  // the next one. Saying "next … 3 minutes ago" would be a next run in the past.
  const due = isDue(schedule.nextRunAt, Date.now());
  return (
    <span
      className={fires ? "tabular-nums" : "text-muted-foreground tabular-nums"}
      title={`${local} (your time)`}
      data-testid='admin-app-schedule-next'
      data-due={due}
    >
      {due ? (
        <>
          {fires ? "due" : "would have been"} {at}{" "}
          <span className='text-muted-foreground'>(reading the next run…)</span>
        </>
      ) : (
        <>
          {fires ? "next" : "would be"} {at}{" "}
          <span className='text-muted-foreground'>({timeAgo(schedule.nextRunAt)})</span>
        </>
      )}
    </span>
  );
};
