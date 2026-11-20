// Figures for a reader: money, percentages, counts, deltas and the missing mark.
//
// Started from the reference app's `src/lib/format.ts` (`usd`, `usdRate`, `pct`), which sets
// the rules it states: a hyphen-minus before the currency mark (`-$1,235`), whole dollars by
// default and cents only when asked, one decimal on a percentage given in points, en-US
// grouping. Where it is silent:
//
// - **Missing value**: an em dash (`—`) for null, undefined, NaN and ±Infinity. The reference
//   uses the em dash for an unreadable month; marketing, supply-chain, accounts-insight,
//   bookkeeping, locations and bmg-oxygen all print it for a missing figure (ads-bidding's en
//   dash is the one outlier), and marketing's `missing()` is the null rule taken here.
// - **Compact money**: lowercase `k`, uppercase `M` and `B`, one decimal under 10k, whole
//   thousands from there, one decimal on millions and billions, a trailing `.0` dropped. That
//   is accounts-insight's `money()` (as BEYOND-GALLERY row 3 picks); its bands are chosen on
//   the value *after* rounding, as quadrogen's `usd` does, so $999,600 reads `$1M`, never
//   `$1000k`.
// - **Zero after rounding has no sign**: `-0.4` is `$0`, not `-$0` (the reference prints
//   `-$0`); a signed delta that rounds to zero is `0%`, not `+0%` (bmg-oxygen's `pct`).
// - **Percent grouping**: none, as the reference's `toFixed` and every percent copy print.
//
// Rounding is half away from zero on the decimal the number prints as (`String(n)`), the way
// Intl and a person round: 1.005 is `1.01` and 2.675 is `2.68`, where `toFixed` gives `1.00`
// and `2.67` because the binary double sits just under the half.
//
// No Intl, no DOM, no Node: the output is the same in a browser, in Node and in an Oxy
// Function isolate, and it does not move when an engine's ICU data changes.

/** The mark for a value that is not there: an em dash. */
export const MISSING = "—";

/** A figure that may be absent: null, undefined and non-finite numbers print {@link MISSING}. */
export type Figure = number | null | undefined;

function isMissing(value: Figure): value is null | undefined {
  return value === null || value === undefined || !Number.isFinite(value);
}

/** `abs` (finite, >= 0) as the decimal it prints as: its digits, and where the point sits. */
function decimal(abs: number, shift: number): { digits: string; point: number } {
  const text = String(abs); // shortest round trip: "1234.5", "1e-7", "1.5e+21"
  const e = text.indexOf("e");
  const mantissa = e < 0 ? text : text.slice(0, e);
  const exponent = e < 0 ? 0 : Number(text.slice(e + 1));
  const dot = mantissa.indexOf(".");
  const digits = dot < 0 ? mantissa : mantissa.slice(0, dot) + mantissa.slice(dot + 1);
  return { digits, point: (dot < 0 ? mantissa.length : dot) + exponent + shift };
}

/** A string of digits plus one, carrying: "129" -> "130", "999" -> "1000", "" -> "1". */
function increment(digits: string): string {
  let i = digits.length - 1;
  while (i >= 0 && digits[i] === "9") i--;
  if (i < 0) return `1${"0".repeat(digits.length)}`;
  return digits.slice(0, i) + String(Number(digits[i]) + 1) + "0".repeat(digits.length - i - 1);
}

/**
 * `abs` (finite, >= 0) times 10^`shift`, rounded half away from zero to `places` decimals.
 * Returns the integer digits (no leading zeros) and exactly `places` fraction digits.
 */
function rounded(abs: number, places: number, shift = 0): { int: string; frac: string } {
  let { digits, point } = decimal(abs, shift);
  if (point < 0) {
    digits = "0".repeat(-point) + digits;
    point = 0;
  }
  const keep = point + places;
  if (digits.length < keep) digits += "0".repeat(keep - digits.length);
  let kept = digits.slice(0, keep);
  if (digits.charCodeAt(keep) >= 53 /* "5" */) kept = increment(kept);
  const intLength = kept.length - places;
  return {
    int: kept.slice(0, intLength).replace(/^0+(?=\d)/, "") || "0",
    frac: kept.slice(intLength)
  };
}

function group(int: string): string {
  return int.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

function isZero(text: string): boolean {
  return !/[1-9]/.test(text);
}

/** The sign a figure carries once its digits are known: none on zero. */
function signFor(value: number, body: string, signed: boolean): string {
  if (isZero(body)) return "";
  if (value < 0) return "-";
  return signed ? "+" : "";
}

function places(digits: number | undefined, fallback: number): number {
  if (digits === undefined || !Number.isFinite(digits)) return fallback;
  return Math.min(20, Math.max(0, Math.floor(digits)));
}

function fixedBody(abs: number, digits: number, grouped: boolean, shift = 0): string {
  const { int, frac } = rounded(abs, digits, shift);
  const whole = grouped ? group(int) : int;
  return frac ? `${whole}.${frac}` : whole;
}

/**
 * The compact body for `abs` that rounds to 1,000 or more, or null below that. Each band is
 * chosen on the value after rounding, so nothing prints as `1000k` or `1000M`.
 */
function compactBody(abs: number): string | null {
  if (rounded(abs, 0).int.length < 4) return null;
  const tenthsOfK = rounded(abs, 1, -3);
  if (tenthsOfK.int.length < 2) return `${trimZero(tenthsOfK)}k`;
  const wholeK = rounded(abs, 0, -3);
  if (wholeK.int.length < 4) return `${wholeK.int}k`;
  const millions = rounded(abs, 1, -6);
  if (millions.int.length < 4) return `${trimZero(millions)}M`;
  const billions = rounded(abs, 1, -9);
  return `${trimZero({ int: group(billions.int), frac: billions.frac })}B`;
}

function trimZero({ int, frac }: { int: string; frac: string }): string {
  return frac === "0" ? int : `${int}.${frac}`;
}

/** Options for {@link formatNumber}. */
export interface NumberFormatOptions {
  /** Decimal places, fixed (0 to 20). Default 0: a whole number. */
  digits?: number;
  /** Put `+` on a positive value (a delta). Zero after rounding stays unsigned. */
  signed?: boolean;
  /**
   * From 1,000 up, the compact form: `1.2k`, `12k`, `3.4M`, `1.2B`. Bands, on the value after
   * rounding: under 999.5 as a plain number; to 9,950 one decimal of k (`.0` dropped); to
   * 999,500 whole k; to 999,950,000 one decimal of M; beyond, one decimal of B.
   */
  compact?: boolean;
}

/**
 * A count or measure with en-US grouping: `1,284`, `-3`, `+12` (signed), `12.5` (`digits: 1`),
 * `34.3k` (compact). {@link MISSING} for null, undefined, NaN and ±Infinity.
 *
 * @example formatNumber(1284) // "1,284"
 * @example formatNumber(-0.4) // "0" (no sign on a zero)
 * @example formatNumber(12, { signed: true }) // "+12"
 */
export function formatNumber(value: Figure, options: NumberFormatOptions = {}): string {
  if (isMissing(value)) return MISSING;
  const abs = Math.abs(value);
  const body =
    (options.compact ? compactBody(abs) : null) ?? fixedBody(abs, places(options.digits, 0), true);
  return `${signFor(value, body, options.signed ?? false)}${body}`;
}

/** Options for {@link formatMoney}. */
export interface MoneyFormatOptions {
  /** Two decimals (`$8.04`), for a rate or a reconciled amount. Default: whole dollars. */
  cents?: boolean;
  /** The compact form from $1,000 up (`$1.2k`, `$80k`, `$3.4M`); bands as {@link NumberFormatOptions.compact}. */
  compact?: boolean;
  /** Put `+` on a positive amount (a delta): `+$1,200`. Zero after rounding stays unsigned. */
  signed?: boolean;
  /**
   * The currency mark, always before the number. Default `$` (US dollars, the reference
   * app's only currency). An app in another currency passes its own, e.g. `CA$` or `€`.
   */
  symbol?: string;
}

/**
 * Money: `$40,499` (whole dollars, the default), `$8.04` (`cents`), `$1.2k` (`compact`),
 * `-$1,235` (the minus before the mark, a hyphen-minus), `+$1,200` (`signed`).
 * {@link MISSING} for null, undefined, NaN and ±Infinity; `-0.4` is `$0`.
 *
 * @example formatMoney(8496.4) // "$8,496"
 * @example formatMoney(12.98, { cents: true }) // "$12.98"
 * @example formatMoney(3_400_000, { compact: true }) // "$3.4M"
 */
export function formatMoney(value: Figure, options: MoneyFormatOptions = {}): string {
  if (isMissing(value)) return MISSING;
  const abs = Math.abs(value);
  const body =
    (options.compact ? compactBody(abs) : null) ?? fixedBody(abs, options.cents ? 2 : 0, true);
  return `${signFor(value, body, options.signed ?? false)}${options.symbol ?? "$"}${body}`;
}

/** Options for {@link formatPercent}. */
export interface PercentFormatOptions {
  /** Decimal places (0 to 20). Default 1, the precision a cost ratio is known to. */
  digits?: number;
  /**
   * The value is a fraction (`0.287` is 28.7%). Default false: the value is already in
   * percentage points (`28.7` is 28.7%), as the reference app's `pct` takes it.
   */
  fraction?: boolean;
  /** Put `+` on a positive value (a change): `+2.5%`. Zero after rounding stays unsigned. */
  signed?: boolean;
}

/**
 * A percentage: `28.7%` from points (the default) or from a fraction (`fraction: true`),
 * `-4.0%`, `+2.5%` (`signed`). No grouping. {@link MISSING} for a missing value. A fraction is
 * scaled on its decimal digits, not by multiplying, so `0.145` at 0 digits is `15%`.
 *
 * @example formatPercent(28.74) // "28.7%"
 * @example formatPercent(0.0123, { fraction: true, digits: 2 }) // "1.23%"
 * @example formatPercent(0.025, { fraction: true, signed: true }) // "+2.5%"
 */
export function formatPercent(value: Figure, options: PercentFormatOptions = {}): string {
  if (isMissing(value)) return MISSING;
  const body = fixedBody(
    Math.abs(value),
    places(options.digits, 1),
    false,
    options.fraction ? 2 : 0
  );
  return `${signFor(value, body, options.signed ?? false)}${body}%`;
}
