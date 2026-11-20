// How long ago: "now", "3m ago", "2h ago", "yesterday", "3d ago", then the date.
//
// The short forms are the SDK inbox's own (`formatAge`, below, moved here from
// `custom-app/notifications.ts`) with "ago" added, as roadmap's `ago` prints them. Under a
// day the count is of elapsed time; from a day on it is of calendar days in the zone, so
// 7 PM the day before is "yesterday" and not "1d ago". A moment in the future (a clock a
// little ahead) reads "now", as `formatAge`, roadmap and accounts-insight all clamp it.

import {
  calendarDaysBetween,
  type DateInput,
  formatDate,
  isCalendarDate,
  readInstant,
  type ZoneOption
} from "./dates";
import { MISSING } from "./numbers";

/**
 * How long ago `at` was, at `now`: `now` (under a minute), `3m ago`, `2h ago` (under a day),
 * `yesterday`, `3d ago` (to six calendar days in the zone), then the date (`Nov 2`, or
 * `Nov 2, 2025` in another year). A calendar date (`"2026-11-18"`) counts days only:
 * `today`, `yesterday`, `3d ago`. {@link MISSING} when either cannot be read.
 *
 * @param at - The moment, or the calendar date, being described.
 * @param now - The present. Required, so the result depends only on the arguments.
 * @param options - `timeZone` for counting calendar days and printing the date; omitted,
 *   the viewer's zone in a browser and UTC in a Function (`defaultTimeZone`).
 *
 * @example formatRelative("2026-11-19T11:57:00Z", "2026-11-19T12:00:00Z") // "3m ago"
 */
export function formatRelative(
  at: DateInput | null | undefined,
  now: DateInput,
  options: ZoneOption = {}
): string {
  const then = readInstant(at);
  const present = readInstant(now);
  if (then === null || present === null || at === null || at === undefined) return MISSING;
  const days = calendarDaysBetween(at, now, options);
  if (days === null) return MISSING;
  if (!isCalendarDate(at)) {
    const seconds = (present - then) / 1000;
    if (seconds < 60) return "now";
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
    if (seconds < 86_400 || days <= 0) return `${Math.floor(seconds / 3600)}h ago`;
  } else if (days <= 0) {
    return "today";
  }
  if (days === 1) return "yesterday";
  if (days < 7) return `${days}d ago`;
  return formatDate(at, { ...options, now, year: "auto" });
}

/**
 * The SDK inbox's age stamp: `now`, `5m`, `3h`, `2d`, then a date in the device's locale and
 * zone. Empty for an unreadable timestamp. Kept exactly as the notifications panel prints it;
 * for new copy prefer {@link formatRelative}, which takes a zone and says "ago".
 */
export function formatAge(iso: string, now: number = Date.now()): string {
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return "";
  const seconds = Math.max(0, (now - at) / 1000);
  if (seconds < 60) return "now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`;
  if (seconds < 7 * 86400) return `${Math.floor(seconds / 86400)}d`;
  return new Date(at).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}
