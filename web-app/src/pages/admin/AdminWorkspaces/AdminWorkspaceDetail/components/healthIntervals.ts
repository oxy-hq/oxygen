import type {
  WorkspaceHealthHistory,
  WorkspaceHealthStatus,
  WorkspaceHealthTransition
} from "@/services/api/workspaceHealth";

/** A stretch of time a workspace spent in one status. */
export interface HealthInterval {
  status: WorkspaceHealthStatus;
  /** When it began, or the window's start if it began before that. */
  start: number;
  /** When it ended; `null` while it is still the workspace's status. */
  end: number | null;
  /** It was already in this status when the window opened, so `start` is the
   *  window's edge and not when it really began. */
  beganBeforeWindow: boolean;
  /** The dimensions failing when it began. */
  failures: string[];
}

const DAY_MS = 86_400_000;

/**
 * Turn changes into the stretches between them, newest first.
 *
 * A change says what a workspace *became*; what an operator asks is how long
 * it *stayed* that way. Each stretch runs from its change to the next one, the
 * newest to now, and the one the window opened in is cut at the window's edge
 * so its length is the time spent inside the window — not since whenever it
 * began.
 *
 * A cut list has no opening stretch. The change before the window did not lead
 * to the oldest change listed — others, dropped, came between — so joining
 * them would draw one long stretch over a period that had several.
 */
export function toIntervals(history: WorkspaceHealthHistory, now: number): HealthInterval[] {
  const windowStart = now - history.window_days * DAY_MS;
  const opening = history.truncated ? null : history.opening;
  const ascending: { change: WorkspaceHealthTransition; opening: boolean }[] = [
    ...(opening ? [{ change: opening, opening: true }] : []),
    ...[...history.transitions].reverse().map((change) => ({ change, opening: false }))
  ];
  return ascending
    .map(({ change, opening }, i) => {
      const next = ascending[i + 1];
      return {
        status: change.to_status,
        start: opening ? windowStart : new Date(change.at).getTime(),
        end: next ? new Date(next.change.at).getTime() : null,
        beganBeforeWindow: opening,
        failures: change.failures.map((f) => f.dimension)
      };
    })
    .reverse();
}

export const lengthOf = (interval: HealthInterval, now: number): number =>
  (interval.end ?? now) - interval.start;

/** `3h 20m`, `2d 4h`, `12m`. Two units at most: an operator reading "was down
 *  for" wants the size of it, not the seconds. */
export function formatSpan(ms: number): string {
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return "under a minute";
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const mins = minutes % 60;
  if (days > 0) return hours > 0 ? `${days}d ${hours}h` : `${days}d`;
  if (hours > 0) return mins > 0 ? `${hours}h ${mins}m` : `${hours}h`;
  return `${mins}m`;
}

const times = (n: number) => (n === 1 ? "once" : n === 2 ? "twice" : `${n} times`);

/**
 * The window in one sentence: how often the workspace was not healthy, and for
 * how long in total.
 *
 * Four different "nothing to report" answers are kept apart. No rows at all
 * means nothing has been recorded — not that the workspace was healthy. A cut
 * list cannot be totalled, so it says so instead of under-counting. And a
 * history that starts inside the window speaks for the part it covers: a
 * workspace first evaluated an hour ago was not "healthy throughout the last
 * 30 days", and one that recovered an hour ago with nothing recorded before
 * that even less so.
 */
export function summarize(history: WorkspaceHealthHistory, now: number): string {
  const intervals = toIntervals(history, now);
  const window = `the last ${history.window_days} days`;
  if (intervals.length === 0) return "No status change has been recorded for this workspace yet.";
  if (history.truncated) {
    return `Changed status more than ${history.transitions.length} times in ${window}; only the most recent are listed, so no total is given.`;
  }
  const oldest = intervals[intervals.length - 1];
  const recorded = oldest.beganBeforeWindow ? null : `${formatSpan(now - oldest.start)} ago`;
  const parts = (["unhealthy", "degraded"] as const)
    .map((status) => {
      const of = intervals.filter((i) => i.status === status);
      if (of.length === 0) return null;
      const total = of.reduce((sum, i) => sum + lengthOf(i, now), 0);
      return `${status} ${times(of.length)}, ${formatSpan(total)} in total`;
    })
    .filter((p): p is string => p !== null);
  const sentence = parts.join(" · ");
  if (recorded) {
    return parts.length === 0
      ? `Healthy since recording began ${recorded}.`
      : `Since recording began ${recorded}: ${sentence}.`;
  }
  return parts.length === 0 ? `Healthy throughout ${window}.` : `In ${window}: ${sentence}.`;
}
