import type { UsageReportRecipient } from "@/types/usageReport";

const ROLE_LABEL: Record<string, string> = {
  global_owner: "Global owner",
  global_admin: "Global admin"
};

/**
 * A recipient's role in words. A role this build has not heard of is shown as the server
 * wrote it: a guess at a nicer name could name the wrong role, and a blank cell names none.
 */
export const roleLabel = (role: string): string => ROLE_LABEL[role] ?? role;

/** How far a recipient's grant reaches. */
export function reachLabel({
  scope_all,
  org_count
}: Pick<UsageReportRecipient, "scope_all" | "org_count">): string {
  if (scope_all) return "All organizations";
  // A scoped grant whose size the server did not say. It is still not "all".
  if (org_count === null) return "Some organizations";
  if (org_count === 0) return "No organizations";
  return `${org_count.toLocaleString("en-US")} ${org_count === 1 ? "organization" : "organizations"}`;
}

/**
 * A timestamp as "Oct 7, 2026", in the reader's own time zone — it says when a person
 * did something, and they did it on the reader's calendar, not on UTC's. `timeZone` is
 * for a caller that needs one particular zone. `null` when there is no usable timestamp.
 */
export function formatDay(iso: string | null, timeZone?: string): string | null {
  if (!iso) return null;
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return null;
  return new Intl.DateTimeFormat("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
    timeZone
  }).format(date);
}

/** One address, however it was typed. */
const sameAddress = (a: string, b: string): boolean =>
  a.trim().toLowerCase() === b.trim().toLowerCase();

/**
 * "Turned off by … on …", for a person whose email someone else switched off — or `null`.
 *
 * Only then. Someone who turned their own email off does not need telling who did it,
 * and a line under every disabled row would bury the rows where it is news: the ones
 * where a person may be wondering why the report stopped arriving.
 */
export function turnedOffBy(
  recipient: Pick<UsageReportRecipient, "email" | "enabled" | "updated_by" | "updated_at">,
  timeZone?: string
): string | null {
  if (recipient.enabled) return null;
  // Never changed: it is on by default, so "off with no one named" should not happen,
  // and if it does there is nobody to name.
  if (!recipient.updated_by) return null;
  if (sameAddress(recipient.updated_by, recipient.email)) return null;
  const day = formatDay(recipient.updated_at, timeZone);
  return day
    ? `Turned off by ${recipient.updated_by} on ${day}`
    : `Turned off by ${recipient.updated_by}`;
}
