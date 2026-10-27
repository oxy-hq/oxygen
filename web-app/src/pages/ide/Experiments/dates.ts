/** `YYYY-MM-DD` and a real day — `2026-02-31` parses in `Date` as March, so round-trip it. */
export function isCalendarDate(value: string): boolean {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
  const parsed = new Date(`${value}T00:00:00Z`);
  return !Number.isNaN(parsed.getTime()) && parsed.toISOString().slice(0, 10) === value;
}

/**
 * The earliest calendar date it currently is anywhere on Earth — today in UTC-12,
 * the last zone to start each day. The list does not carry an experiment's
 * `design.timezone`, so a date before this is in the past in *every* zone and is
 * the only "past" the form can call: the exact check in the design's own zone is
 * the server's. `en-CA` formats as `YYYY-MM-DD`.
 */
export function earliestTodayOnEarth(now: Date): string {
  return new Intl.DateTimeFormat("en-CA", {
    timeZone: "Etc/GMT+12",
    year: "numeric",
    month: "2-digit",
    day: "2-digit"
  }).format(now);
}

/** An RFC 3339 instant as `YYYY-MM-DD HH:MM UTC`; an unparsable value is returned as given. */
export function formatInstant(iso: string): string {
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return iso;
  return `${parsed.toISOString().slice(0, 16).replace("T", " ")} UTC`;
}
