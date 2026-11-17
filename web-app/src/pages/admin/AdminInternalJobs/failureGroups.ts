import type { FailureGroup } from "@/services/api/internalJobs";

/**
 * What the "Failing now" section says. Pure, so the sentence and the tone are decided
 * once and tested — the same reason `AdminHome/findings.ts` exists.
 */

const HOUR_MS = 60 * 60 * 1000;

/** A cause still producing failures within this long reads as ongoing, not historic. */
export const ONGOING_WITHIN_MS = 24 * HOUR_MS;

/**
 * Danger when any job in the group exhausted its retries — nothing will try again — or the
 * cause is still firing; warn for a group that has gone quiet but is still in the window.
 */
export function groupTone(g: FailureGroup, now = Date.now()): "danger" | "warn" {
  if (g.dead > 0) return "danger";
  return now - new Date(g.last_seen).getTime() <= ONGOING_WITHIN_MS ? "danger" : "warn";
}

/** `3 causes behind 1,214 failures in the last 7 days.` */
export function groupsSentence(groups: FailureGroup[], days: number): string {
  if (groups.length === 0) return `Nothing has failed in the last ${days} days.`;
  const total = groups.reduce((s, g) => s + g.occurrences, 0);
  const causes = groups.length === 1 ? "1 cause" : `${groups.length} causes`;
  const failures = total === 1 ? "1 failure" : `${total.toLocaleString("en-US")} failures`;
  return `${causes} behind ${failures} in the last ${days} days.`;
}

/**
 * How long a cause has been failing, in words an operator acts on: "once", or "for 18
 * days". A cause that has fired for weeks is a different problem from one that started
 * an hour ago, and the two counts can be equal.
 */
export function spanPhrase(g: FailureGroup): string {
  if (g.occurrences === 1) return "once";
  const span = new Date(g.last_seen).getTime() - new Date(g.first_seen).getTime();
  if (span < HOUR_MS) return "within the hour";
  if (span < 24 * HOUR_MS) return `over ${Math.round(span / HOUR_MS)}h`;
  const days = Math.round(span / (24 * HOUR_MS));
  return `over ${days} day${days === 1 ? "" : "s"}`;
}

/** The text the console's search should hold to show this group's jobs. */
export function groupQuery(g: FailureGroup): string {
  return g.workspace_name ?? g.task_type;
}
