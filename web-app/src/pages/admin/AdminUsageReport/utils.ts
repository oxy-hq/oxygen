import ROUTES from "@/libs/utils/routes";
import type { UsageHighlight, UsageHighlightKind } from "@/types/usageReport";

/** One app's console, where a highlight or a table row sends the reader. */
export const appConsolePath = (orgSlug: string, appSlug: string): string =>
  `${ROUTES.ADMIN.CUSTOMER_APPS}/${orgSlug}/${appSlug}`;

export const formatCount = (n: number): string => n.toLocaleString("en-US");

export const plural = (n: number, one: string, many: string): string => (n === 1 ? one : many);

const KB = 1024;
const MB = KB * 1024;
const GB = MB * 1024;
const ONE_DECIMAL = new Intl.NumberFormat("en-US", {
  minimumFractionDigits: 1,
  maximumFractionDigits: 1
});

/**
 * A size in bytes, as "512 B", "12 KB", "3.4 MB" or "3.4 GB".
 *
 * Base 1024, with one decimal from MB up — below that a decimal is noise beside sizes
 * that run to gigabytes. It stops at GB: a terabyte reads "1,024.0 GB", which keeps a
 * column of sizes in one unit at the top end, where they are compared.
 *
 * Each step rounds first and checks the ceiling second, so a size just under a boundary
 * reads "1.0 MB" and never "1,024 KB".
 */
export function formatBytes(bytes: number): string {
  // Also catches NaN and a negative size, neither of which is a size.
  if (!(bytes > 0)) return "0 B";
  if (bytes < KB) return `${formatCount(Math.round(bytes))} B`;
  const kb = Math.round(bytes / KB);
  if (kb < 1024) return `${formatCount(kb)} KB`;
  const mb = Math.round((bytes / MB) * 10) / 10;
  if (mb < 1024) return `${ONE_DECIMAL.format(mb)} MB`;
  return `${ONE_DECIMAL.format(bytes / GB)} GB`;
}

/**
 * This week against the week before, as a signed count: "+3", "−2", "0".
 *
 * A real minus sign (U+2212) rather than a hyphen, so it is as wide as the plus and the
 * column lines up under `tabular-nums`.
 */
export function formatChange(current: number, previous: number): string {
  const delta = current - previous;
  if (delta > 0) return `+${formatCount(delta)}`;
  if (delta < 0) return `−${formatCount(-delta)}`;
  return "0";
}

// UTC on purpose: a period is Monday 00:00 UTC to the next Monday, and formatting it in
// the reader's zone would start the week on a Sunday for anyone west of Greenwich.
const DAY = new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
const DAY_AND_YEAR = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
  year: "numeric",
  timeZone: "UTC"
});

/**
 * The week a report covers, as "Sep 28 – Oct 4, 2026".
 *
 * `periodEnd` is exclusive, so the last day shown is the one before it. `null` when
 * either timestamp does not parse — the caller leaves the line out rather than printing
 * "Invalid Date" above a report that is otherwise fine.
 */
export function formatPeriod(periodStart: string, periodEnd: string): string | null {
  const start = new Date(periodStart);
  // The last moment the period includes.
  const last = new Date(new Date(periodEnd).getTime() - 1);
  if (Number.isNaN(start.getTime()) || Number.isNaN(last.getTime())) return null;
  const sameYear = start.getUTCFullYear() === last.getUTCFullYear();
  return `${(sameYear ? DAY : DAY_AND_YEAR).format(start)} – ${DAY_AND_YEAR.format(last)}`;
}

const HIGHLIGHT_LABEL: Record<UsageHighlightKind, string> = {
  went_quiet: "Went quiet",
  dropping: "Fewer people",
  failing_functions: "Functions failing",
  client_errors: "Page errors",
  growing: "More people",
  first_week: "First week",
  unused: "Not opened"
};

/**
 * The short name for a highlight's kind.
 *
 * A kind this build has not heard of is spelled out rather than left blank: the server
 * writes the report and can add a kind before the console ships its label, and a row
 * with no label reads as a rendering bug.
 */
export function highlightLabel(kind: string): string {
  const known = HIGHLIGHT_LABEL[kind as UsageHighlightKind];
  if (known) return known;
  const words = kind.replaceAll("_", " ").trim();
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : "Highlight";
}

export interface HighlightGroups {
  attention: UsageHighlight[];
  good: UsageHighlight[];
  idle: UsageHighlight[];
}

/**
 * Highlights split by tone, each group in the order the server sent it.
 *
 * Anything that is not plainly `good` or `idle` lands under attention. A tone this build
 * does not know is something the server thought worth saying, and filing it under "needs
 * a look" costs a glance where dropping it would cost the finding.
 */
export function highlightsByTone(highlights: readonly UsageHighlight[]): HighlightGroups {
  const groups: HighlightGroups = { attention: [], good: [], idle: [] };
  for (const highlight of highlights) {
    if (highlight.tone === "good") groups.good.push(highlight);
    else if (highlight.tone === "idle") groups.idle.push(highlight);
    else groups.attention.push(highlight);
  }
  return groups;
}
