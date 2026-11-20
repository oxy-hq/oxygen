// Started from oxy-hq/reference-app@3caad1f `src/lib/tableSort.ts` (INVENTORY section 3: the
// reference's, tested, and copied with its tests into watchlist, marketing, supply-chain and BMG
// ads-bidding; every copy is the same code with other comments). Kept whole; nothing is new.
//
// Sorting a table by its column headings.
//
// **The one rule that matters here: a row with nothing in the sorted column goes to the END, in
// both directions.** A table usually opens on an order chosen to keep unjudgeable rows off the
// top (no books connected, not linked to the feed). A naive comparator undoes that the first time
// someone clicks a heading: nulls become zero, zero sorts first ascending, and the top of the
// table fills with the rows that were deliberately kept out of it. So absence is not a value here
// and never sorts against one. It is a third group, always last.
//
// Everything in this file is pure so it can be tested; the screen holds the state and the URL
// carries it.

export type SortDir = "asc" | "desc";

export interface SortState<K extends string = string> {
  key: K;
  dir: SortDir;
}

/**
 * What clicking a heading does.
 *
 * Three states, not two: the heading you are already sorting by cycles through its two
 * directions and then back to **no sort at all**, which restores the table's own default order.
 * A default order often carries a judgement no single column can reproduce (dollars, then the
 * rows flagged only on rating, then alphabetical), so without a way back a reader would lose it
 * on their first click.
 */
export function nextSort<K extends string>(
  current: SortState<K> | null,
  key: K,
  firstDir: SortDir
): SortState<K> | null {
  if (!current || current.key !== key) return { key, dir: firstDir };
  if (current.dir === firstDir) return { key, dir: firstDir === "asc" ? "desc" : "asc" };
  return null;
}

/**
 * Rows in order, with the valueless ones last.
 *
 * The rows that DO have a value are sorted; the rows that do not keep the order they arrived in,
 * so the tail is still whatever the table's default put there. `Array.prototype.sort` is stable
 * (ES2019), so equal values also keep their incoming order and no tiebreak is needed: pass rows
 * already in the default order and ties fall back to it.
 */
export function sortRows<T>(
  rows: readonly T[],
  valueFor: (row: T) => number | string | null,
  dir: SortDir
): T[] {
  const valued: { row: T; v: number | string }[] = [];
  const empty: T[] = [];
  for (const row of rows) {
    const v = valueFor(row);
    // NaN is absence too: it arrives from arithmetic on a missing figure, and every comparison
    // with it is false, so its place would depend on the engine rather than on the data.
    if (v === null || (typeof v === "number" && !Number.isFinite(v))) empty.push(row);
    else valued.push({ row, v });
  }
  const sign = dir === "asc" ? 1 : -1;
  valued.sort((a, b) =>
    typeof a.v === "number" && typeof b.v === "number"
      ? sign * (a.v - b.v)
      : sign * String(a.v).localeCompare(String(b.v))
  );
  return [...valued.map((x) => x.row), ...empty];
}

/**
 * `?sort=food_cost` ascending, `?sort=-food_cost` descending, absent for the table's own order.
 * The leading minus keeps the default out of the URL: a link with no params means "whatever you
 * would show me today".
 */
export function formatSortParam(s: SortState | null): string | null {
  return s ? (s.dir === "desc" ? `-${s.key}` : s.key) : null;
}

/** One of `allowed`, or null. A URL is untrusted input; anything else is absent. */
export function parseSortParam<K extends string>(
  raw: string | null,
  allowed: readonly K[]
): SortState<K> | null {
  if (!raw) return null;
  const dir: SortDir = raw.startsWith("-") ? "desc" : "asc";
  const key = raw.startsWith("-") ? raw.slice(1) : raw;
  return (allowed as readonly string[]).includes(key) ? { key: key as K, dir } : null;
}
