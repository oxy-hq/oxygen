/**
 * Log axes for the cost-against-runs map.
 *
 * Both axes are logarithmic because the fleet spans orders of magnitude: one
 * tenant at $1.8k and a long tail at single dollars cannot share a linear axis
 * without the tail collapsing into one corner. On log–log axes a constant
 * cost per run is a straight diagonal, which is what makes the guides work.
 */

export type LogDomain = readonly [number, number];

/** Decade bounds that cover every positive value; `null` when there are none. */
export function logDomain(values: number[]): LogDomain | null {
  const positive = values.filter((v) => v > 0 && Number.isFinite(v));
  if (positive.length === 0) return null;
  const lo = 10 ** Math.floor(Math.log10(Math.min(...positive)));
  let hi = 10 ** Math.ceil(Math.log10(Math.max(...positive)));
  // One value sitting exactly on a decade would give a zero-width axis.
  if (hi <= lo) hi = lo * 10;
  return [lo, hi];
}

/** Where `v` falls along the axis, 0 at the low end and 1 at the high end. */
export function logPosition(v: number, [lo, hi]: LogDomain): number {
  return (Math.log10(v) - Math.log10(lo)) / (Math.log10(hi) - Math.log10(lo));
}

/** One tick per decade, inclusive of both ends. */
export function decadeTicks([lo, hi]: LogDomain): number[] {
  const ticks: number[] = [];
  for (let k = Math.round(Math.log10(lo)); k <= Math.round(Math.log10(hi)); k += 1) {
    ticks.push(10 ** k);
  }
  return ticks;
}

/** Round figures a reader can hold in mind as "dollars per run". */
export const PER_RUN_STEPS = [
  0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1, 2, 5, 10, 20, 50, 100
] as const;

/**
 * Three cost-per-run guides centred on the fleet's typical org: the step
 * nearest its median, and the steps two either side (roughly ×5 and ÷5).
 *
 * Centred on the data rather than fixed, so a fleet that runs at cents per
 * run and one that runs at dollars both get guides that cross the plot.
 */
export function perRunGuides(costsPerRun: number[]): number[] {
  const positive = costsPerRun.filter((v) => v > 0 && Number.isFinite(v)).sort((a, b) => a - b);
  if (positive.length === 0) return [];
  const mid = Math.floor(positive.length / 2);
  const median =
    positive.length % 2 === 0 ? (positive[mid - 1] + positive[mid]) / 2 : positive[mid];
  let nearest = 0;
  for (let i = 1; i < PER_RUN_STEPS.length; i += 1) {
    const d = Math.abs(Math.log10(PER_RUN_STEPS[i]) - Math.log10(median));
    if (d < Math.abs(Math.log10(PER_RUN_STEPS[nearest]) - Math.log10(median))) nearest = i;
  }
  return [nearest - 2, nearest, nearest + 2]
    .filter((i) => i >= 0 && i < PER_RUN_STEPS.length)
    .map((i) => PER_RUN_STEPS[i]);
}

/** `$1k`, `$10`, `$0.1` — an axis label, not a figure. */
export function axisUsd(v: number): string {
  if (v >= 1_000_000) return `$${v / 1_000_000}M`;
  if (v >= 1_000) return `$${v / 1_000}k`;
  return `$${v}`;
}

/** `10k`, `100`, `1` — a count on an axis. */
export function axisCount(v: number): string {
  if (v >= 1_000_000) return `${v / 1_000_000}M`;
  if (v >= 1_000) return `${v / 1_000}k`;
  return `${v}`;
}

/** `$0.50 / run` — a guide's label. */
export function perRunLabel(v: number): string {
  const amount = v >= 1 ? `$${v}` : `$${v.toFixed(v < 0.01 ? 3 : 2)}`;
  return `${amount} / run`;
}
