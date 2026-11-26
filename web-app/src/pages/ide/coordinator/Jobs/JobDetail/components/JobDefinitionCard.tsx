import type React from "react";
import { useMemo } from "react";
import { Link } from "react-router-dom";
import { Badge } from "@/components/ui/shadcn/badge";
import type { Schedule } from "@/types/schedule";
import { useCoordinatorRoutes } from "../../../components/useCoordinatorRoutes";
import { cronNextRuns, describeCron, formatTimestamp, shortId } from "../../../components/utils";

const Field: React.FC<{ label: string; children: React.ReactNode }> = ({ label, children }) => (
  <div className='flex flex-col gap-0.5'>
    <span className='text-muted-foreground text-xs uppercase tracking-wide'>{label}</span>
    <div className='text-sm'>{children}</div>
  </div>
);

/** The static side of a job: target, cadence, and its next fire times. */
export const JobDefinitionCard: React.FC<{ schedule: Schedule }> = ({ schedule }) => {
  const routes = useCoordinatorRoutes();
  const nextRuns = useMemo(
    () => cronNextRuns(schedule.cron_expr, schedule.timezone, 5),
    [schedule.cron_expr, schedule.timezone]
  );

  return (
    <div className='border border-border bg-card'>
      <div className='border-border border-b px-3 py-2'>
        <h3 className='font-semibold text-sm'>Definition</h3>
      </div>
      <div className='grid grid-cols-2 gap-4 p-3'>
        <Field label='Target'>
          <div className='flex items-center gap-2'>
            <Badge variant='secondary'>{schedule.target_kind}</Badge>
            <span className='truncate font-mono text-xs'>{schedule.target_ref}</span>
          </div>
        </Field>
        <Field label='Timezone'>{schedule.timezone}</Field>
        {schedule.target_kind === "agent" && schedule.question && (
          <div className='col-span-2'>
            <Field label='Question'>
              <p className='whitespace-pre-wrap text-sm'>{schedule.question}</p>
            </Field>
          </div>
        )}
        <Field label='Schedule'>
          <code className='font-mono'>{schedule.cron_expr}</code>
          <span className='ml-2 text-muted-foreground text-xs'>
            {describeCron(schedule.cron_expr)}
          </span>
        </Field>
        <Field label='Last run'>
          {schedule.last_run_id ? (
            <Link
              to={routes.RUN_DETAIL(schedule.last_run_id)}
              className='font-mono text-primary text-xs hover:underline'
            >
              {shortId(schedule.last_run_id)}
            </Link>
          ) : (
            <span className='text-muted-foreground'>Never run</span>
          )}
          <span className='ml-2 text-muted-foreground text-xs'>
            {formatTimestamp(schedule.last_fired_at)}
          </span>
        </Field>
        <div className='col-span-2'>
          <Field label='Next 5 runs'>
            {schedule.enabled ? (
              <ol className='mt-1 flex flex-col gap-0.5'>
                {nextRuns.length === 0 ? (
                  <li className='text-muted-foreground text-xs'>No upcoming runs</li>
                ) : (
                  nextRuns.map((d, i) => (
                    <li
                      key={d.toISOString()}
                      className='flex items-center gap-2 text-xs tabular-nums'
                    >
                      <span className='w-4 text-muted-foreground'>{i + 1}.</span>
                      <span>
                        {d.toLocaleString(undefined, {
                          weekday: "short",
                          month: "short",
                          day: "numeric",
                          hour: "2-digit",
                          minute: "2-digit"
                        })}
                      </span>
                    </li>
                  ))
                )}
              </ol>
            ) : (
              <span className='text-muted-foreground text-xs'>
                Job is paused — enable it to resume the schedule.
              </span>
            )}
          </Field>
        </div>
      </div>
    </div>
  );
};
