import dayjs from "dayjs";
import duration from "dayjs/plugin/duration";
import relativeTime from "dayjs/plugin/relativeTime";

dayjs.extend(relativeTime);
dayjs.extend(duration);

/**
 * A server timestamp as a `Date`, or `null` when there is nothing usable.
 *
 * The API serves ISO-8601 UTC, but not always with its zone designator: a
 * naive `2026-09-28T10:00:00` handed to `new Date` is read as LOCAL time, which
 * shifts every "last compiled" by the viewer's offset. A string without a zone
 * is therefore read as UTC. Anything unparseable yields `null` rather than an
 * `Invalid Date` that renders as "NaN years ago".
 */
export function parseUtcTimestamp(value: string | null | undefined): Date | null {
  if (!value) return null;
  const trimmed = value.trim();
  const hasTime = /T|\d \d/.test(trimmed);
  const hasZone = /(Z|[+-]\d{2}:?\d{2})$/i.test(trimmed);
  const normalized = hasTime && !hasZone ? `${trimmed.replace(" ", "T")}Z` : trimmed;
  const date = new Date(normalized);
  return Number.isNaN(date.getTime()) ? null : date;
}

/** "3 minutes ago" for an already-parsed instant (see `parseUtcTimestamp`). */
export function dateAgo(date: Date): string {
  return dayjs(date).fromNow();
}

export function timeAgo(dateString: string): string {
  return dayjs(dateString).fromNow();
}

export function formatDate(date: string) {
  return new Date(date).toLocaleDateString("en-US", {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit"
  });
}

const MS_PER_DAY = 1000 * 60 * 60 * 24;

export function formatRelativeDate(dateStr: string | null | undefined): string | null {
  if (!dateStr) return null;
  const date = new Date(dateStr);
  const diffDays = Math.floor((Date.now() - date.getTime()) / MS_PER_DAY);
  if (diffDays === 0) return "Today";
  if (diffDays === 1) return "Yesterday";
  if (diffDays < 7) return `${diffDays} days ago`;
  return date.toLocaleDateString("en-US", { month: "short", day: "numeric" });
}
