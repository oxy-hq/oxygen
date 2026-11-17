import { PRODUCTION } from "@/types/appEnvironments";
import type { AppFunctionSummary, FunctionInvocation } from "@/types/apps";

/**
 * The pure half of the Schedules panel: which functions are on a timer, whether
 * the environment fires them, and the last time one fired.
 *
 * There is no per-schedule pause anywhere in the platform. A schedule fires when
 * its function declares it in the build the environment serves, and — outside
 * production — when that environment's switch (`app_environments.schedules_enabled`)
 * is on. Production has no switch: its schedules always run.
 */

/** The timezone a schedule row gets when the manifest names none
 *  (`custom_apps_env_schedules::reconcile`). */
export const DEFAULT_TIMEZONE = "UTC";

export interface ScheduledFunction {
  name: string;
  cron: string;
  timezone: string;
  /** The manifest named the timezone (else it is the UTC default). */
  timezoneDeclared: boolean;
  /** The server's reading of the cron: the next fire after now, or null. */
  nextRunAt: string | null;
}

/** The functions that declare a schedule, soonest next run first; one the
 *  scheduler cannot read (no next run) last. */
export function scheduledFunctions(fns: readonly AppFunctionSummary[]): ScheduledFunction[] {
  return fns
    .filter((fn): fn is AppFunctionSummary & { schedule: string } => !!fn.schedule)
    .map((fn) => ({
      name: fn.name,
      cron: fn.schedule,
      timezone: fn.timezone ?? DEFAULT_TIMEZONE,
      timezoneDeclared: !!fn.timezone,
      nextRunAt: fn.next_run_at ?? null
    }))
    .sort((a, b) => {
      if (a.nextRunAt && b.nextRunAt && a.nextRunAt !== b.nextRunAt) {
        return Date.parse(a.nextRunAt) - Date.parse(b.nextRunAt);
      }
      if (!!a.nextRunAt !== !!b.nextRunAt) return a.nextRunAt ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
}

/** How long after the soonest next run the functions are read again: long enough
 *  for the server's clock, which computes the next one, to be past it too. */
export const NEXT_RUN_GRACE_MS = 5_000;
/** Never re-read sooner than this, so a browser clock ahead of the server's cannot
 *  spin the panel; never wait longer than this, which also keeps the wait inside
 *  `setTimeout`'s range for a monthly cron. */
export const NEXT_RUN_REFRESH_FLOOR_MS = 30_000;
export const NEXT_RUN_REFRESH_CEILING_MS = 60 * 60_000;

/** The next run has passed: the server read the cron before that instant, so the
 *  fire is due or under way, and the value no longer says when the next one is. */
export function isDue(nextRunAt: string | null, now: number): boolean {
  if (!nextRunAt) return false;
  const at = Date.parse(nextRunAt);
  return !Number.isNaN(at) && at <= now;
}

/** How long to wait before reading the functions again, so no row keeps a next run
 *  that has passed: just after the soonest one, within the floor and ceiling.
 *  `null` when no schedule has a next run to wait for. */
export function nextRunRefreshMs(nextRuns: readonly (string | null)[], now: number): number | null {
  const times = nextRuns
    .map((at) => (at ? Date.parse(at) : Number.NaN))
    .filter((at) => !Number.isNaN(at));
  if (times.length === 0) return null;
  const wait = Math.min(...times) - now + NEXT_RUN_GRACE_MS;
  return Math.min(NEXT_RUN_REFRESH_CEILING_MS, Math.max(NEXT_RUN_REFRESH_FLOOR_MS, wait));
}

/** Whether the environment fires its schedules. */
export type ScheduleSwitch =
  /** Production: no switch, always on. */
  | { state: "always" }
  | { state: "on" }
  | { state: "off" }
  /** Not known: still loading, refused (no non-production reach), or failed. */
  | { state: "unknown"; why: "loading" | "forbidden" | "missing" | "error" };

/** The switch from the environment's schedules read. `status` is the HTTP status
 *  of a failed read, when it got one. */
export function scheduleSwitch(
  environment: string,
  read: { enabled?: boolean; pending: boolean; status?: number; failed: boolean }
): ScheduleSwitch {
  if (environment === PRODUCTION) return { state: "always" };
  if (read.enabled !== undefined) return { state: read.enabled ? "on" : "off" };
  if (read.failed) {
    if (read.status === 403) return { state: "unknown", why: "forbidden" };
    if (read.status === 404) return { state: "unknown", why: "missing" };
    return { state: "unknown", why: "error" };
  }
  return { state: "unknown", why: "loading" };
}

/** The newest cron fire among `rows`: mode `schedule` only — a Run now, even one
 *  run as the schedule, is recorded `manual` and is not a fire. */
export function lastScheduledRun<T extends Pick<FunctionInvocation, "mode" | "created_at">>(
  rows: readonly T[]
): T | null {
  let newest: T | null = null;
  for (const iv of rows) {
    if (iv.mode !== "schedule") continue;
    if (!newest || Date.parse(iv.created_at) > Date.parse(newest.created_at)) newest = iv;
  }
  return newest;
}

/** "Tue 18 Nov, 09:00" as the clock in `timeZone` reads it — the zone the cron is
 *  written in, so it matches the expression beside it. */
export function formatInZone(iso: string, timeZone: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "—";
  try {
    return new Intl.DateTimeFormat("en-GB", {
      timeZone,
      weekday: "short",
      day: "numeric",
      month: "short",
      hour: "2-digit",
      minute: "2-digit",
      hour12: false
    }).format(at);
  } catch {
    // An unknown zone: the scheduler could not read it either (no next run), but
    // a stray instant still renders.
    return at.toISOString();
  }
}
