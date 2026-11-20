// Copy helpers: a count and its noun, an English list, a capital first letter.
//
// From the reference app's `src/lib/format.ts:93-151` (`countPhrase`, `joinWords`,
// `sentenceCase`; watchlist and the QSR template copies are identical), with three fixes:
// `countPhrase` spelled a negative or fractional count as "undefined" (it indexed the word
// table with it); `joinNames` here is the reference's `joinWords`, because the reference's
// `joinNames` also strips a store-chain prefix, which is app data; and a count from 1,000
// is grouped, as the brief for this entry asks of every count. `plural` is store-ops'
// `lib/plural.ts` (and warehouse's `fmtCount`): always digits, for a caption or a label.

import { formatNumber, MISSING } from "./numbers";

const SMALL = ["no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"];

function noun(n: number, singular: string, plural: string): string {
  return n === 1 ? singular : plural;
}

function digits(n: number): string {
  if (!Number.isFinite(n)) return MISSING;
  return Number.isInteger(n) ? formatNumber(n) : String(n);
}

/**
 * A count in running text, small counts as words: `no stores`, `one store`, `nine stores`,
 * `23 stores`, `1,284 stores`. Words for whole numbers 0 to 10; digits otherwise (a negative
 * or a fraction too). The noun is singular only for exactly 1.
 *
 * @example sentenceCase(countPhrase(3, "guest")) // "Three guests"
 */
export function countPhrase(n: number, singular: string, plural = `${singular}s`): string {
  const word = Number.isInteger(n) && n >= 0 && n <= 10 ? SMALL[n] : digits(n);
  return `${word} ${noun(n, singular, plural)}`;
}

/**
 * A count and its noun, always in digits: `0 documents`, `1 document`, `1,284 documents`.
 * For a caption or a label, where a word would be slower to scan than a digit.
 */
export function plural(n: number, singular: string, many = `${singular}s`): string {
  return `${digits(n)} ${noun(n, singular, many)}`;
}

/** Options for {@link joinNames}. */
export interface JoinOptions {
  /** The word before the last item. Default `and`. */
  conjunction?: string;
}

/**
 * An English list, not a CSV: `Birchwood`, `Birchwood and Dunmore`, `Birchwood, Dunmore and
 * Elmwood` (no serial comma, as the reference writes it). Blank entries are dropped; an empty
 * list is the empty string.
 */
export function joinNames(names: readonly string[], options: JoinOptions = {}): string {
  const items = names.filter((name) => name.trim() !== "");
  const and = options.conjunction ?? "and";
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} ${and} ${items[items.length - 1]}`;
}

/** The first letter up, the rest as it was: `three guests` -> `Three guests`. */
export function sentenceCase(text: string): string {
  const first = text.codePointAt(0);
  if (first === undefined) return text;
  const char = String.fromCodePoint(first);
  return char.toUpperCase() + text.slice(char.length);
}
