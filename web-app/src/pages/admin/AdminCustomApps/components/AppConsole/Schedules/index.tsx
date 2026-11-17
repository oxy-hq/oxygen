import { isAxiosError } from "axios";
import { useEffect } from "react";
import { useAppFunctions } from "@/hooks/api/customApps/useAppFunctions";
import {
  useAppInvocationsMany,
  useEnvironmentSchedules
} from "@/hooks/api/customApps/useAppInvocations";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import type { CustomApp } from "@/types/apps";
import { environmentLabel } from "../../AppDetail/components/Secrets/environmentLabel";
import { ConsolePanel } from "../../ConsolePanel";
import { ScheduleRow } from "./ScheduleRow";
import {
  lastScheduledRun,
  nextRunRefreshMs,
  type ScheduleSwitch,
  scheduledFunctions,
  scheduleSwitch
} from "./schedules";

/**
 * The console environment's scheduled functions: each one's cron and zone, when it
 * fires next, its last cron fire, and Run now as the schedule.
 *
 * Above them, whether the environment fires schedules at all. That switch is per
 * environment and is the only one there is: nothing pauses one schedule. It is
 * shown, not flipped, here.
 */
export const SchedulesPanel = ({
  app,
  environment,
  scope
}: {
  app: CustomApp;
  environment: string;
  scope: { label: string; warn?: boolean };
}) => {
  const functions = useAppFunctions(app.id, environment);
  const switchRead = useEnvironmentSchedules(app.id, environment);
  const schedules = scheduledFunctions(functions.data ?? []);
  // `next_run_at` is the server's reading at the moment it answered, and nothing
  // else refetches the list. Read it again just after the soonest one passes, so
  // a row shows "due" for a few seconds rather than a next run in the past.
  // Re-arm after every completed read, not only when a value changes: a server
  // whose clock is behind the browser's can answer with the same passed instant,
  // and a failed read changes nothing either. While a run is still past, the
  // floor in `nextRunRefreshMs` bounds how often this reads.
  const { refetch, dataUpdatedAt, errorUpdatedAt } = functions;
  const nextRuns = schedules.map((s) => s.nextRunAt ?? "").join(" ");
  // biome-ignore lint/correctness/useExhaustiveDependencies: the read times re-arm the timer; they are not read inside it
  useEffect(() => {
    const wait = nextRunRefreshMs(nextRuns.split(" "), Date.now());
    if (wait == null) return;
    const timer = setTimeout(() => void refetch(), wait);
    return () => clearTimeout(timer);
  }, [nextRuns, refetch, dataUpdatedAt, errorUpdatedAt]);
  // One read per function: the newest fire of each, however busy the others are.
  const lastFires = useAppInvocationsMany(
    app.id,
    schedules.map((s) => ({ environment, function: s.name, mode: "schedule", limit: 1 }))
  );
  const fired = scheduleSwitch(environment, {
    enabled: switchRead.data?.enabled,
    pending: switchRead.isPending,
    failed: switchRead.isError,
    status: isAxiosError(switchRead.error) ? switchRead.error.response?.status : undefined
  });
  const where = environmentLabel(environment).toLowerCase();

  return (
    <ConsolePanel
      id='schedules'
      title='Schedules'
      question='Q1 — what runs on a timer'
      scope={scope}
    >
      <AdminAsync
        query={functions}
        noun='this app&rsquo;s functions'
        rows={2}
        isEmpty={() => schedules.length === 0}
        empty={
          <p className='text-muted-foreground text-xs' data-testid='admin-app-schedules-empty'>
            No function declares a <code>schedule</code> in the build {where} serves.
          </p>
        }
      >
        {() => (
          <div className='flex flex-col gap-2' data-testid='admin-app-schedules'>
            <SwitchNote fired={fired} label={environmentLabel(environment)} />
            <ul className='flex flex-col'>
              {schedules.map((schedule, i) => {
                const last = lastFires[i];
                return (
                  <ScheduleRow
                    key={schedule.name}
                    app={app}
                    environment={environment}
                    schedule={schedule}
                    fires={fired.state === "always" || fired.state === "on"}
                    last={last?.data ? lastScheduledRun(last.data.invocations) : null}
                    lastPending={!!last?.isPending}
                  />
                );
              })}
            </ul>
          </div>
        )}
      </AdminAsync>
    </ConsolePanel>
  );
};

/** Whether this environment fires the schedules below, in one sentence. */
const SwitchNote = ({ fired, label }: { fired: ScheduleSwitch; label: string }) => {
  const where = label.toLowerCase();
  const sentence = (() => {
    switch (fired.state) {
      case "always":
        return "Production has no schedules switch: every schedule below fires.";
      case "on":
        return `Schedules are on in ${where}: every schedule below fires, against ${where}'s data.`;
      case "off":
        return `Schedules are off in ${where} (the default): none below fires until staff turn the environment's switch on. Run now still runs one.`;
      default:
        return fired.why === "forbidden"
          ? `You may not read ${where}'s schedules switch.`
          : fired.why === "missing"
            ? `${label} has no environment row yet, so it has no switch to read.`
            : fired.why === "loading"
              ? "Reading the schedules switch…"
              : "Couldn't read the schedules switch.";
    }
  })();
  return (
    <p className='text-xs' data-testid='admin-app-schedules-switch' data-state={fired.state}>
      {sentence}{" "}
      <span className='text-muted-foreground'>
        The switch is per environment; no single schedule can be paused.
      </span>
    </p>
  );
};
