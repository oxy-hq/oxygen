/**
 * An experiment's grain (`design.grain`): the period one panel cell covers. Every
 * window count in a design is a number of these, and every date a design names
 * opens one — a week is Monday to Sunday, a month starts on the 1st.
 */
export type Grain = "hour" | "day" | "week" | "month";

/** Every grain, as the picker lists them. Hour is offered only on a datetime calendar: see `grainChoices`. */
export const GRAIN_CHOICES: { value: Grain; label: string }[] = [
  { value: "hour", label: "Hour" },
  { value: "day", label: "Day" },
  { value: "week", label: "Week (Monday to Sunday)" },
  { value: "month", label: "Month" }
];

export function grainOf(value: unknown): Grain {
  return value === "hour" || value === "week" || value === "month" ? value : "day";
}

/** What the time-dimensions endpoint returns: date/datetime ids per view, and which are datetime. */
export interface CalendarDims {
  by_view: Record<string, string[]>;
  datetime?: string[];
}

/**
 * The grains the picker offers for this outcome: hour only when the calendar
 * (`time_dimension`, else the outcome view's only date dimension) is a datetime —
 * a date column has no hour to read. A design already at `current` keeps it listed.
 */
export function grainChoices(
  target: string,
  timeDimension: string,
  dims: CalendarDims | undefined,
  current?: Grain
) {
  const view = target.split(".")[0];
  const own = dims?.by_view[view] ?? [];
  const calendar = timeDimension || (own.length === 1 ? own[0] : "");
  const hourly = calendar !== "" && (dims?.datetime ?? []).includes(calendar);
  return GRAIN_CHOICES.filter((c) => c.value !== "hour" || hourly || current === "hour");
}

const parse = (iso: string) => new Date(`${iso}T00:00:00Z`);

/** Whether `date` (`YYYY-MM-DD`) is the first day of its period. */
export function isPeriodStart(date: string, grain: Grain): boolean {
  const d = parse(date);
  if (Number.isNaN(d.getTime())) return false;
  if (grain === "week") return d.getUTCDay() === 1;
  if (grain === "month") return d.getUTCDate() === 1;
  return true;
}

/** The input a switch moment is entered in: a date, or a local date and time at hour grain. */
export function switchInputType(grain: Grain): "date" | "datetime-local" {
  return grain === "hour" ? "datetime-local" : "date";
}

const LOCAL_HOUR = /^(\d{4}-\d{2}-\d{2})T(\d{2}):(\d{2})(:00)?$/;

/** The date part of a switch moment (`2026-10-26T14:00` → `2026-10-26`). */
export function switchDate(value: string): string {
  return value.slice(0, 10);
}

/**
 * Whether `value` is a switch moment this grain takes: a date that starts a period
 * (a Monday, the 1st), or at hour grain a date or a local whole hour (`2026-10-26T14:00`).
 */
export function isSwitchMoment(value: string, grain: Grain): boolean {
  const m = LOCAL_HOUR.exec(value);
  if (m) return grain === "hour" && m[3] === "00" && Number(m[2]) < 24;
  return isPeriodStart(value, grain);
}

/** "Pick a Monday." — what a date must be at this grain; empty at day grain. */
export function periodStartRule(grain: Grain): string {
  if (grain === "hour")
    return "Pick a whole hour, such as 14:00: at hour grain a switch is on the hour.";
  if (grain === "week") return "Pick a Monday: at week grain every date starts a week.";
  if (grain === "month")
    return "Pick the 1st of a month: at month grain every date starts a month.";
  return "";
}

export function periodWord(grain: Grain, o: { plural?: boolean; capital?: boolean } = {}): string {
  const word = `${grain}${o.plural ? "s" : ""}`;
  return o.capital ? word[0].toUpperCase() + word.slice(1) : word;
}

/** `3 weeks`, `1 month`. */
export function periods(n: number, grain: Grain): string {
  return `${n} ${periodWord(grain, { plural: n !== 1 })}`;
}
