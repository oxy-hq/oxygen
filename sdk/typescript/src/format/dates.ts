// Dates and times for a reader: the forms the apps print, and the freshness wording.
//
// Started from the reference app's `src/lib/format.ts` (`monthYearLabel`, `shortMonthLabel`,
// `monthDay`), whose rules hold here: month names in English, the em dash for a month it
// cannot read. Where it is silent:
//
// - **A calendar date (`2026-11-19`) is never moved by a time zone.** It is split by hand, as
//   locations' `dayLabel`, warehouse's `fmtDateOnly` and store-ops' `digest.ts` all insist:
//   `new Date("2026-11-19")` is UTC midnight, the evening before anywhere west of Greenwich.
// - **An instant is shown in the zone passed; with none, in the viewer's own zone in a browser
//   and in UTC in an Oxy Function** (decision 65, `defaultTimeZone`): the isolate has no
//   `document`, and its own zone is the server's, which no reader shares. marketing,
//   supply-chain and bookkeeping pin `America/Los_Angeles` and warehouse refuses the device's
//   zone, so pass the business's zone when a figure belongs to a place. A date-time string
//   with no zone is read as UTC, never as local.
// - **Forms**: `Nov 19` and `Nov 19, 2026` (supply-chain, warehouse, marketing, roadmap; the
//   year only when it is not `now`'s, as supply-chain and bookkeeping do), `Nov 19, 3:03 PM`
//   (marketing and supply-chain `fmtStamp`), `3:03 PM` with a plain space before AM/PM,
//   `Wed, Nov 19` (the comma Intl's en-US weekday form prints), and ranges `Sep 21 – 27, 2026`
//   / `Sep 28 – Oct 4, 2026` / `Dec 28, 2026 – Jan 3, 2027` (store-ops `digest.ts weekLabel`).
// - **Freshness**: "As of 3:03 PM" today, "As of Nov 18, 3:03 PM" another day (decision 48's
//   wording), "Data through Nov 18" for the newest complete day.
//
// The words come from the tables below, not from Intl, so the output does not change with an
// engine's ICU data (newer ICU puts U+202F before "PM"). Intl is used only to read the wall
// clock of a named zone; the Oxy Function isolate (bare `deno_core` on V8) carries Intl with
// its zone data, so a named zone works there too.

import { MISSING } from "./numbers";

/** An instant (`Date`, epoch milliseconds, ISO string) or a calendar date (`"2026-11-19"`). */
export type DateInput = Date | number | string;

/** Where an instant's wall clock is read. */
export interface ZoneOption {
  /**
   * An IANA zone (`"America/Los_Angeles"`), `"UTC"` or `"local"` (the device's own zone).
   * Omitted, {@link defaultTimeZone} decides: the viewer's zone in a browser, UTC in a
   * Function. A calendar date (`"2026-11-19"`) is never moved by it. An unknown zone throws a
   * `RangeError`, as Intl does.
   */
  timeZone?: string;
}

/** Whether a date names its year. */
export interface YearOption {
  /**
   * `"auto"` (the default) names the year only when it is not `now`'s year, and always when
   * there is no `now`; `true` always; `false` never.
   */
  year?: boolean | "auto";
  /** The present, for `year: "auto"` (and for the freshness wording's "today"). */
  now?: DateInput;
}

/** Options for the date forms. */
export interface DateFormatOptions extends ZoneOption, YearOption {}

/** A wall-clock reading: month 1-12, weekday 0 (Sunday) to 6. */
interface Wall {
  y: number;
  m: number;
  d: number;
  dow: number;
  hour: number;
  minute: number;
  /** True for a calendar date, which has no time of day. */
  dateOnly: boolean;
}

const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December"
] as const;
const WEEKDAYS = [
  "Sunday",
  "Monday",
  "Tuesday",
  "Wednesday",
  "Thursday",
  "Friday",
  "Saturday"
] as const;

const DATE_ONLY = /^(\d{4})-(\d{2})-(\d{2})$/;
const MONTH_ONLY = /^(\d{4})-(\d{2})$/;
const DAY_MS = 86_400_000;

/** Days since 1970-01-01 for a calendar date (any year, unlike `Date.UTC`'s 0-99 quirk). */
function dayNumber(y: number, m: number, d: number): number {
  const t = new Date(0);
  t.setUTCFullYear(y, m - 1, d);
  return Math.floor(t.getTime() / DAY_MS);
}

function calendarDay(y: number, m: number, d: number): Wall | null {
  if (m < 1 || m > 12 || d < 1) return null;
  const t = new Date(0);
  t.setUTCFullYear(y, m - 1, d);
  if (t.getUTCDate() !== d) return null; // 2026-02-30 rolled into March
  return { y, m, d, dow: t.getUTCDay(), hour: 0, minute: 0, dateOnly: true };
}

function isDate(value: unknown): value is Date {
  return Object.prototype.toString.call(value) === "[object Date]";
}

/**
 * Epoch milliseconds for an instant, or null when it cannot be read. Accepts Postgres's text
 * form (`2026-10-20 02:35:00+00`); a date-time with no zone is UTC.
 */
export function readInstant(input: DateInput | null | undefined): number | null {
  if (isDate(input)) return Number.isNaN(input.getTime()) ? null : input.getTime();
  if (typeof input === "number") return Number.isFinite(input) ? input : null;
  if (typeof input !== "string") return null;
  let text = input.trim();
  if (!text) return null;
  text = text
    .replace(/^(\d{4}-\d{2}-\d{2}) (?=\d)/, "$1T")
    .replace(/(T\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?[+-]\d{2})$/, "$1:00");
  if (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d+)?)?$/.test(text)) text += "Z";
  const t = Date.parse(text);
  return Number.isNaN(t) ? null : t;
}

const zoneReaders = new Map<string, Intl.DateTimeFormat>();

function zoneReader(timeZone: string): Intl.DateTimeFormat {
  let reader = zoneReaders.get(timeZone);
  if (!reader) {
    reader = new Intl.DateTimeFormat("en-US", {
      timeZone,
      hourCycle: "h23",
      year: "numeric",
      month: "numeric",
      day: "numeric",
      hour: "numeric",
      minute: "numeric"
    });
    zoneReaders.set(timeZone, reader);
  }
  return reader;
}

function wallAt(t: number, timeZone: string): Wall {
  const at = new Date(t);
  if (timeZone === "UTC") {
    const [y, m, d] = [at.getUTCFullYear(), at.getUTCMonth() + 1, at.getUTCDate()];
    return {
      y,
      m,
      d,
      dow: at.getUTCDay(),
      hour: at.getUTCHours(),
      minute: at.getUTCMinutes(),
      dateOnly: false
    };
  }
  if (timeZone === "local") {
    const [y, m, d] = [at.getFullYear(), at.getMonth() + 1, at.getDate()];
    return {
      y,
      m,
      d,
      dow: at.getDay(),
      hour: at.getHours(),
      minute: at.getMinutes(),
      dateOnly: false
    };
  }
  const parts = zoneReader(timeZone).formatToParts(at);
  const part = (type: Intl.DateTimeFormatPartTypes) =>
    Number(parts.find((p) => p.type === type)?.value);
  const [y, m, d] = [part("year"), part("month"), part("day")];
  const dow = new Date(dayNumber(y, m, d) * DAY_MS).getUTCDay();
  return { y, m, d, dow, hour: part("hour") % 24, minute: part("minute"), dateOnly: false };
}

/** The wall-clock reading of `input` in the zone, or null when it cannot be read. */
function wallOf(input: DateInput | null | undefined, options: ZoneOption): Wall | null {
  if (typeof input === "string") {
    const day = DATE_ONLY.exec(input.trim());
    if (day) return calendarDay(Number(day[1]), Number(day[2]), Number(day[3]));
  }
  const t = readInstant(input);
  return t === null ? null : wallAt(t, options.timeZone ?? defaultTimeZone());
}

/**
 * The zone an instant is shown in when no `timeZone` is passed (decision 65): `"local"`, the
 * viewer's own, where there is a `document` (a browser); `"UTC"` where there is none (an Oxy
 * Function, whose isolate's own zone is the server's). Decided on every call, never cached.
 */
export function defaultTimeZone(): "local" | "UTC" {
  return typeof document === "undefined" ? "UTC" : "local";
}

/** Like {@link wallOf}, but also reads a month id (`"2026-07"`) as that month's first day. */
function monthWallOf(input: DateInput | null | undefined, options: ZoneOption): Wall | null {
  const month = typeof input === "string" ? MONTH_ONLY.exec(input.trim()) : null;
  return month ? calendarDay(Number(month[1]), Number(month[2]), 1) : wallOf(input, options);
}

function namesYear(y: number, options: DateFormatOptions): boolean {
  const year = options.year ?? "auto";
  if (year !== "auto") return year;
  if (options.now === undefined) return true;
  const now = wallOf(options.now, options);
  return now === null || now.y !== y;
}

const shortMonth = (w: Wall) => MONTHS[w.m - 1].slice(0, 3);
const monthAndDay = (w: Wall) => `${shortMonth(w)} ${w.d}`;
const clock = (w: Wall) =>
  `${w.hour % 12 || 12}:${String(w.minute).padStart(2, "0")} ${w.hour < 12 ? "AM" : "PM"}`;

/** Options for {@link formatDate}. */
export interface ShortDateOptions extends DateFormatOptions {
  /** Lead with the short weekday: `Wed, Nov 19`. */
  weekday?: boolean;
}

/**
 * `Nov 19`, or `Nov 19, 2026` when the year is not `now`'s (see {@link YearOption}); with
 * `weekday`, `Wed, Nov 19`. {@link MISSING} for anything unreadable.
 *
 * @example formatDate("2026-11-19", { now: "2026-01-02" }) // "Nov 19"
 * @example formatDate("2025-11-19", { now: "2026-01-02" }) // "Nov 19, 2025"
 */
export function formatDate(
  input: DateInput | null | undefined,
  options: ShortDateOptions = {}
): string {
  const w = wallOf(input, options);
  if (!w) return MISSING;
  const lead = options.weekday ? `${WEEKDAYS[w.dow].slice(0, 3)}, ` : "";
  return `${lead}${monthAndDay(w)}${namesYear(w.y, options) ? `, ${w.y}` : ""}`;
}

/** `2026-07` (or any date in it) as `July 2026`. {@link MISSING} for anything unreadable. */
export function formatMonthYear(
  input: DateInput | null | undefined,
  options: ZoneOption = {}
): string {
  const w = monthWallOf(input, options);
  return w ? `${MONTHS[w.m - 1]} ${w.y}` : MISSING;
}

/** `2026-07` (or any date in it) as `Jul`, for a column whose year is stated elsewhere. */
export function formatShortMonth(
  input: DateInput | null | undefined,
  options: ZoneOption = {}
): string {
  const w = monthWallOf(input, options);
  return w ? shortMonth(w) : MISSING;
}

/** `2026-07-16` as `July 16`: the long month, no year. {@link MISSING} for anything unreadable. */
export function formatMonthDay(
  input: DateInput | null | undefined,
  options: ZoneOption = {}
): string {
  const w = wallOf(input, options);
  return w ? `${MONTHS[w.m - 1]} ${w.d}` : MISSING;
}

/** The weekday: `Wed`, or `Wednesday` with `long`. {@link MISSING} for anything unreadable. */
export function formatWeekday(
  input: DateInput | null | undefined,
  options: ZoneOption & { long?: boolean } = {}
): string {
  const w = wallOf(input, options);
  if (!w) return MISSING;
  return options.long ? WEEKDAYS[w.dow] : WEEKDAYS[w.dow].slice(0, 3);
}

/** The time of day: `3:03 PM`, `12:00 AM`. A calendar date has none: {@link MISSING}. */
export function formatTime(input: DateInput | null | undefined, options: ZoneOption = {}): string {
  const w = wallOf(input, options);
  return w && !w.dateOnly ? clock(w) : MISSING;
}

/**
 * `Nov 19, 3:03 PM`, or `Nov 19, 2025, 3:03 PM` when the year is not `now`'s. A calendar date
 * prints as {@link formatDate} does. {@link MISSING} for anything unreadable.
 */
export function formatDateTime(
  input: DateInput | null | undefined,
  options: DateFormatOptions = {}
): string {
  const w = wallOf(input, options);
  if (!w) return MISSING;
  const date = `${monthAndDay(w)}${namesYear(w.y, options) ? `, ${w.y}` : ""}`;
  return w.dateOnly ? date : `${date}, ${clock(w)}`;
}

/**
 * A span of days: `Sep 21 – 27, 2026`, `Sep 28 – Oct 4, 2026`, `Dec 28, 2026 – Jan 3, 2027`
 * (an en dash between spaces). One day prints once. The year follows {@link YearOption}, and a
 * span across years always names both.
 */
export function formatDateRange(
  start: DateInput | null | undefined,
  end: DateInput | null | undefined,
  options: DateFormatOptions = {}
): string {
  const a = wallOf(start, options);
  const b = wallOf(end, options);
  if (!a || !b) return MISSING;
  if (a.y !== b.y) return `${monthAndDay(a)}, ${a.y} – ${monthAndDay(b)}, ${b.y}`;
  const year = namesYear(a.y, options) ? `, ${a.y}` : "";
  if (a.m === b.m && a.d === b.d) return `${monthAndDay(a)}${year}`;
  if (a.m === b.m) return `${monthAndDay(a)} – ${b.d}${year}`;
  return `${monthAndDay(a)} – ${monthAndDay(b)}${year}`;
}

/** Options for the freshness wording. */
export interface FreshnessOptions extends ZoneOption {
  /** The present: an instant on `now`'s day reads as a time alone, and `now`'s year is not named. */
  now?: DateInput;
}

/**
 * When the figures were read: `As of 3:03 PM` on `now`'s day, `As of Nov 18, 3:03 PM` on
 * another, `As of Nov 18, 2025, 3:03 PM` in another year; a calendar date, `As of Nov 18`.
 * Without `now` the date and year are always named. {@link MISSING} for anything unreadable.
 */
export function formatAsOf(
  input: DateInput | null | undefined,
  options: FreshnessOptions = {}
): string {
  const w = wallOf(input, options);
  if (!w) return MISSING;
  const now = options.now === undefined ? null : wallOf(options.now, options);
  const today = now !== null && !w.dateOnly && now.y === w.y && now.m === w.m && now.d === w.d;
  return `As of ${today ? clock(w) : formatDateTime(input, { ...options, year: "auto" })}`;
}

/**
 * The newest day the figures cover: `Data through Nov 18`, or `Data through Nov 18, 2025` when
 * that is not `now`'s year (always, without `now`). {@link MISSING} for anything unreadable.
 */
export function formatDataThrough(
  input: DateInput | null | undefined,
  options: FreshnessOptions = {}
): string {
  const date = formatDate(input, { ...options, year: "auto" });
  return date === MISSING ? MISSING : `Data through ${date}`;
}

/** Calendar days from `then` to `now`, each read in the zone. Internal to the relative form. */
export function calendarDaysBetween(
  then: DateInput,
  now: DateInput,
  options: ZoneOption
): number | null {
  const a = wallOf(then, options);
  const b = wallOf(now, options);
  if (!a || !b) return null;
  return dayNumber(b.y, b.m, b.d) - dayNumber(a.y, a.m, a.d);
}

/** True for a calendar-date string (`"2026-11-19"`), which has no time of day. */
export function isCalendarDate(input: unknown): input is string {
  return typeof input === "string" && DATE_ONLY.test(input.trim());
}
