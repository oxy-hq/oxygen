// Started from pokehouse-oxy `apps/watchlist/src/components/TrendChart.tsx:82-137`
// (`labelledIndices`) and `src/lib/format.ts:118-165` (`axisTickLabels` and the month labels it
// uses). INVENTORY section 8, surprise 3: the reference's month labels can collide, Watchlist
// fixed it and tested it (`TrendChart.test.ts`, ported beside TrendChart here). Kept unchanged:
// the position-aware thinning (first and last always, a label between only where it clears its
// predecessor AND the last), the widths measured at 12px in the system face, the year on the first
// tick and on each new year's. New: the month words are an argument (`format`), English by default.

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
