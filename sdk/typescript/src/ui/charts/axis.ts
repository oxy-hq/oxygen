// Started from pokehouse-oxy `apps/watchlist/src/components/TrendChart.tsx:82-137`
// (`labelledIndices`) and `src/lib/format.ts:118-165` (`axisTickLabels` and the month labels it
// uses). INVENTORY section 8, surprise 3: the reference's month labels can collide, Watchlist
// fixed it and tested it (`TrendChart.test.ts`, ported beside TrendChart here). Kept unchanged:
// the position-aware thinning (first and last always, a label between only where it clears its
// predecessor AND the last), the widths measured at 12px in the system face, the year on the first
// tick and on each new year's. New: the month words are an argument (`format`), English by default.
//
// 2.31.1: `valueAxis`, the y scale and its three labels, which never print the same label twice.

/** What the y-axis measures: a percentage (whole-percent labels) or a rating (one decimal). */
export type ValueUnit = "percent" | "stars";

/** The labels' decimals: as drawn since 2.30.0, and the finer step (the readout's own, "24.6%",
 *  "4.46") they take where two would otherwise print the same. */
const TICK_DECIMALS: Record<ValueUnit, readonly [coarse: number, fine: number]> = {
  percent: [0, 1],
  stars: [1, 2]
};
/** A flat series: a scale this far either side of its value. */
const FLAT_SPREAD: Record<ValueUnit, number> = { percent: 1, stars: 0.1 };
/** The narrowest scale: two fine steps between neighbouring gridlines, so their labels always
 *  differ at the fine step. */
const MIN_SPAN: Record<ValueUnit, number> = { percent: 0.4, stars: 0.04 };

export interface ValueAxis {
  /** The scale, low to high: the values' own extent, unless flat or too narrow to label. */
  domain: [number, number];
  /** The three gridlines' values: the low end, the middle, the high end. */
  ticks: [number, number, number];
  /** Their labels, low to high, no two the same. */
  labels: [string, string, string];
  /** Whether the labels took the finer step (wider: "25.4%" where "25%" was). */
  fine: boolean;
}

function tickLabel(value: number, unit: ValueUnit, decimals: number): string {
  if (unit === "percent" && decimals === 0) return `${Math.round(value)}%`;
  // A value that rounds to nothing reads "0.0", not "-0.0".
  const text = value.toFixed(decimals).replace(/^-(?=0(?:\.0*)?$)/, "");
  return unit === "percent" ? `${text}%` : text;
}

/**
 * The y scale for `values` and its three gridlines' labels. The scale is the values' own extent;
 * a flat series gets one around its value; one narrower than `MIN_SPAN` is widened to it around
 * its middle (from zero, for data that is not below it), so the labels can always differ. They are
 * whole percents, or a rating's one decimal, as before; where two of those would print the same,
 * all three take one more decimal. Wherever the coarse labels already differed, the scale and the
 * labels are what they were.
 */
export function valueAxis(values: readonly number[], unit: ValueUnit): ValueAxis {
  const lo = values.length ? Math.min(...values) : 0;
  const hi = values.length ? Math.max(...values) : 0;
  let domain: [number, number];
  if (!(hi > lo)) domain = [lo - FLAT_SPREAD[unit], hi + FLAT_SPREAD[unit]];
  else if (hi - lo < MIN_SPAN[unit]) {
    const from = Math.max((lo + hi) / 2 - MIN_SPAN[unit] / 2, lo >= 0 ? 0 : -Infinity);
    domain = [from, from + MIN_SPAN[unit]];
  } else domain = [lo, hi];
  const ticks: [number, number, number] = [domain[0], (domain[0] + domain[1]) / 2, domain[1]];
  const [coarse, fine] = TICK_DECIMALS[unit];
  const at = (decimals: number) =>
    ticks.map((t) => tickLabel(t, unit, decimals)) as [string, string, string];
  const labels = at(coarse);
  if (new Set(labels).size === labels.length) return { domain, ticks, labels, fine: false };
  return { domain, ticks, labels: at(fine), fine: true };
}

/** The month labels' rhythm: two labels' centres at least this far apart, in SVG units. */
const X_LABEL_W = 64;
/** What a month label takes at 12px, in CSS px, with a margin over the widest measured in the
 *  system face in Chrome ("May 2026" 56.5px, "May" 23.4px). */
const X_LABEL_W_YEAR = 60;
const X_LABEL_W_MONTH = 26;
/** Clear space between two month labels' ink, in CSS px. */
const X_LABEL_GAP = 6;

/**
 * Which months get a label on a `plotW`-wide axis: the first and the last, and between them only
 * a label that clears both its predecessor and the last one, by centres (the rhythm) and by its
 * own extent (a year-bearing label is twice a bare month's width; the first is anchored at its
 * start, the last at its end). `unitsPerPx` turns the labels' CSS-px widths into SVG units: 1
 * where the chart draws 1:1, under 1 where a fixed viewBox is drawn wider than it is.
 */
export function labelledIndices(
  periods: readonly string[],
  plotW: number,
  unitsPerPx = 1
): Set<number> {
  const n = periods.length;
  if (n <= 1) return new Set([0]);
  const x = (i: number) => (i * plotW) / (n - 1);
  const yearOf = (i: number) => periods[i]?.slice(0, 4) ?? "";
  const width = (yearBearing: boolean) =>
    (yearBearing ? X_LABEL_W_YEAR : X_LABEL_W_MONTH) * unitsPerPx;
  const gap = X_LABEL_GAP * unitsPerPx;
  const lastX = x(n - 1);
  const out = new Set<number>([0, n - 1]);
  let prevX = 0;
  let prevRight = width(true);
  let prevYear = yearOf(0);
  for (let i = 1; i < n - 1; i++) {
    const xi = x(i);
    if (xi - prevX < X_LABEL_W) continue;
    if (lastX - xi < X_LABEL_W) break;
    const half = width(yearOf(i) !== prevYear) / 2;
    if (xi - half < prevRight + gap) continue;
    // The last label carries its year when this, the last kept before it, is in another year.
    if (xi + half + gap > lastX - width(yearOf(n - 1) !== yearOf(i))) continue;
    out.add(i);
    prevX = xi;
    prevRight = xi + half;
    prevYear = yearOf(i);
  }
  return out;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** `2026-07` as "Jul", or "Jul 2026" with its year. A dash for anything unparseable. */
export function monthLabel(period: string, withYear: boolean): string {
  const m = /^(\d{4})-(\d{2})/.exec(period);
  const month = m ? MONTHS[Number(m[2]) - 1] : undefined;
  if (!m || !month) return "—";
  return withYear ? `${month} ${m[1]}` : month;
}

/**
 * The tick text for the months that get a tick: the year only on the first tick and on the
 * first tick of each new year ("Jun 2026", "Jul", "Aug", "Jan 2027"). The year change is judged
 * against the previous TICK, so thinning the axis never drops a year boundary.
 */
export function axisTickLabels(
  periods: readonly string[],
  shown: Iterable<number>,
  format: (period: string, withYear: boolean) => string = monthLabel
): Map<number, string> {
  const out = new Map<number, string>();
  let prevYear: string | null = null;
  for (const i of [...shown].sort((a, b) => a - b)) {
    const id = periods[i] ?? "";
    const year = /^(\d{4})/.exec(id)?.[1] ?? null;
    out.set(i, format(id, year !== null && year !== prevYear));
    prevYear = year ?? prevYear;
  }
  return out;
}
