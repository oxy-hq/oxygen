import { isAxiosError } from "axios";
import { useEffect } from "react";
import { useAppFunctions } from "@/hooks/api/customApps/useAppFunctions";
import {
  useAppInvocationsMany,
  useEnvironmentSchedules
} from "@/hooks/api/customApps/useAppInvocations";
import { cn } from "@/libs/shadcn/utils";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { AdminInfo } from "@/pages/admin/components/AdminInfo";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
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

  return (
    <ConsolePanel
      id='schedules'
      title='Schedules'
      info="Functions that run on a timer in this environment's build, each with its next and last run. Whether they fire is the environment's schedules switch: production always fires; other environments are off until staff turn the switch on."
      scope={scope}
    >
      <AdminAsync
        query={functions}
        noun='this app&rsquo;s functions'
        rows={2}
        isEmpty={() => schedules.length === 0}
        empty={
          <p className='text-muted-foreground text-xs' data-testid='admin-app-schedules-empty'>
            No schedules in this build.
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

/**
 * Whether this environment fires the schedules below: a dot and a few words, the full
 * sentence behind an ⓘ. It used to be two sentences above the list on every visit.
 */
const SwitchNote = ({ fired, label }: { fired: ScheduleSwitch; label: string }) => {
  const where = label.toLowerCase();
  const { short, tone, sentence } = (() => {
    switch (fired.state) {
      case "always":
        return {
          short: "Every schedule fires",
          tone: "ok" as const,
          sentence: "Production has no schedules switch: every schedule below fires."
        };
      case "on":
        return {
          short: "Switch on · schedules fire",
          tone: "ok" as const,
          sentence: `Schedules are on in ${where}: every schedule below fires, against ${where}'s data.`
        };
      case "off":
        return {
          short: "Switch off · nothing fires",
          tone: "muted" as const,
          sentence: `Schedules are off in ${where} (the default): none below fires until staff turn the environment's switch on. Run now still runs one.`
        };
      default:
        return {
          short: fired.why === "loading" ? "Reading the switch…" : "Switch unknown",
          tone: "muted" as const,
          sentence:
            fired.why === "forbidden"
              ? `You may not read ${where}'s schedules switch.`
              : fired.why === "missing"
                ? `${label} has no environment row yet, so it has no switch to read.`
                : fired.why === "loading"
                  ? "Reading the schedules switch…"
                  : "Couldn't read the schedules switch."
        };
    }
  })();
  return (
    <p
      className='flex items-center gap-1.5 text-muted-foreground text-xs'
      data-testid='admin-app-schedules-switch'
      data-state={fired.state}
    >
      <span className={cn("size-1.5 shrink-0 rounded-full", ADMIN_TONE[tone].dot)} />
      <span>{short}</span>
      <AdminInfo>
        {sentence} The switch is per environment; no single schedule can be paused.
      </AdminInfo>
    </p>
  );
};
