/**
 * `@oxy-hq/sdk/format` — how a custom app writes figures, dates and lists for a reader.
 *
 * Pure functions, no React, no DOM and no Node built-ins, so the same import works in a
 * browser and inside an Oxy Function (the isolate has no `Buffer`, `process` or
 * `TextEncoder`; it does have `Intl`, which this entry uses only to read a named zone's wall
 * clock). Every result depends on the arguments and one fact about the runtime: a helper
 * that compares with the present takes `now`, and an instant is shown in the zone you name,
 * or with none in the viewer's own zone in a browser and UTC in a Function
 * ({@link defaultTimeZone}, decision 65).
 *
 * The house rules, stated once:
 * - the missing-value mark is an em dash, {@link MISSING} (`—`);
 * - the minus is a hyphen-minus before the currency mark (`-$1,235`), and a value that rounds
 *   to zero carries no sign;
 * - money is US dollars by default, whole dollars unless `cents`;
 * - compact figures: `$950`, `$1.2k`, `$80k`, `$3.4M`, `$1.2B`, bands chosen after rounding;
 * - a calendar date (`"2026-11-19"`) is never shifted by a time zone.
 *
 * `formatAge` (the notifications panel's stamp) lives here too; its old import from the
 * root entry's notifications module keeps working.
 *
 * @packageDocumentation
 */

export { type CsvCell, type CsvColumn, csvCell, toCsv } from "./csv";
export {
  type DateFormatOptions,
  type DateInput,
  defaultTimeZone,
  type FreshnessOptions,
  formatAsOf,
  formatDataThrough,
  formatDate,
  formatDateRange,
  formatDateTime,
  formatMonthDay,
  formatMonthYear,
  formatShortMonth,
  formatTime,
  formatWeekday,
  type ShortDateOptions,
  type YearOption,
  type ZoneOption
} from "./dates";
export {
  type Figure,
  formatMoney,
  formatNumber,
  formatPercent,
  MISSING,
  type MoneyFormatOptions,
  type NumberFormatOptions,
  type PercentFormatOptions
} from "./numbers";
export { formatAge, formatRelative } from "./relative";
export { countPhrase, type JoinOptions, joinNames, plural, sentenceCase } from "./words";
