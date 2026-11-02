import type { GlobalNotificationSettings, QuietHours } from "@/types/notifications";

/** Pause for a while. `until` is computed when picked, in this browser's local time. */
export const PAUSE_PRESETS: { id: string; label: string; until: (now: Date) => Date }[] = [
  { id: "hour", label: "1 hour", until: (now) => new Date(now.getTime() + 60 * 60 * 1000) },
  {
    id: "tomorrow",
    label: "Until tomorrow 8:00",
    until: (now) => {
      const at = new Date(now);
      at.setDate(at.getDate() + 1);
      at.setHours(8, 0, 0, 0);
      return at;
    }
  },
  {
    id: "week",
    label: "1 week",
    until: (now) => {
      const at = new Date(now);
      at.setDate(at.getDate() + 7);
      return at;
    }
  }
];

/** When the pause ends, or `null` when nothing is paused (never set, or already over). */
export function pausedUntil(global: GlobalNotificationSettings, now: Date): Date | null {
  if (!global.paused_until) return null;
  const until = new Date(global.paused_until);
  return Number.isNaN(until.getTime()) || until <= now ? null : until;
}

const sameDay = (a: Date, b: Date) =>
  a.getFullYear() === b.getFullYear() &&
  a.getMonth() === b.getMonth() &&
  a.getDate() === b.getDate();

/** "3:45 PM", "tomorrow 8:00 AM", "Sat 8:00 AM", or "Oct 24, 3:45 PM" — as near as it is. */
export function formatUntil(until: Date, now: Date): string {
  const time = until.toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" });
  if (sameDay(until, now)) return time;
  const tomorrow = new Date(now);
  tomorrow.setDate(tomorrow.getDate() + 1);
  if (sameDay(until, tomorrow)) return `tomorrow ${time}`;
  if (until.getTime() - now.getTime() < 6 * 24 * 60 * 60 * 1000) {
    return `${until.toLocaleDateString("en-US", { weekday: "short" })} ${time}`;
  }
  return `${until.toLocaleDateString("en-US", { month: "short", day: "numeric" })}, ${time}`;
}

/** Quiet hours as they start out: overnight, every day, critical only. */
export const DEFAULT_QUIET_HOURS: QuietHours = {
  start: "22:00",
  end: "07:00",
  days: [0, 1, 2, 3, 4, 5, 6],
  allow: "critical"
};

/** 0 = Sunday, as the server counts. */
export const DAYS: { value: number; short: string; label: string }[] = [
  { value: 0, short: "S", label: "Sunday" },
  { value: 1, short: "M", label: "Monday" },
  { value: 2, short: "T", label: "Tuesday" },
  { value: 3, short: "W", label: "Wednesday" },
  { value: 4, short: "T", label: "Thursday" },
  { value: 5, short: "F", label: "Friday" },
  { value: 6, short: "S", label: "Saturday" }
];

const TIME = /^([01]\d|2[0-3]):[0-5]\d$/;

/** What is wrong with `quiet`, in a few words, or `null`. The server checks the same. */
export function quietHoursError(quiet: QuietHours): string | null {
  if (!TIME.test(quiet.start) || !TIME.test(quiet.end)) return "Use a time like 22:00";
  if (quiet.start === quiet.end) return "Start and end must differ";
  if (quiet.days.length === 0) return "Pick at least one day";
  return null;
}

/** This browser's IANA time zone; UTC when it will not say. */
function browserTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
}

/**
 * What saving `quiet` writes. Quiet hours need a time zone, so the first save
 * also stores this browser's; after that the stored one is kept.
 */
export function quietHoursChange(
  quiet: QuietHours | null,
  global: GlobalNotificationSettings
): Partial<GlobalNotificationSettings> {
  if (quiet && !global.timezone) return { quiet_hours: quiet, timezone: browserTimeZone() };
  return { quiet_hours: quiet };
}
