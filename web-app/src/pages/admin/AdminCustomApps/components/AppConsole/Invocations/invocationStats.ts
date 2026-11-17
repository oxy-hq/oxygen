import type { AppInvocationList, FunctionInvocation } from "@/types/apps";

/**
 * The pure half of the invocation explorer: the window it reads, the latency
 * distribution it draws, and the count it compares with the window before.
 *
 * The list is capped by the server (`MAX_LIMIT`), and the distribution is drawn
 * from that list, so it is "of the newest N" whenever the window holds more. The
 * counts are not: the server counts the whole window beside the capped page.
 */

/** The windows the explorer offers. */
export const WINDOWS = {
  "24h": { label: "24h", ms: 24 * 60 * 60 * 1000 },
  "7d": { label: "7 days", ms: 7 * 24 * 60 * 60 * 1000 }
} as const;
export type WindowKey = keyof typeof WINDOWS;

/** The most rows one read returns: the server's `invocations::MAX_LIMIT`. */
export const INVOCATION_CAP = 200;

/** A window ending at `anchor`, and the equal window just before it. Half-open
 *  like the server's `since`/`until`, so the two never share a row. */
export function windowRange(
  window: WindowKey,
  anchor: number
): { since: string; until: string; previousSince: string; previousUntil: string } {
  const span = WINDOWS[window].ms;
  const iso = (ms: number) => new Date(ms).toISOString();
  return {
    since: iso(anchor - span),
    until: iso(anchor),
    previousSince: iso(anchor - 2 * span),
    previousUntil: iso(anchor - span)
  };
}

/**
 * The `p`th percentile of `values` by nearest rank: the smallest value at least
 * `p`% of the values are at or below. Always a value that occurred — never an
 * interpolation between two — and `null` for no values.
 */
export function percentile(values: readonly number[], p: number): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const rank = Math.ceil((Math.min(100, Math.max(0, p)) / 100) * sorted.length);
  return sorted[Math.max(0, rank - 1)];
}

export interface LatencyBucket {
  /** Inclusive lower edge, ms. */
  from: number;
  /** Exclusive upper edge, ms (inclusive for the last bucket). */
  to: number;
  count: number;
}

export interface LatencyDistribution {
  buckets: LatencyBucket[];
  /** The axis, ms: where the first bucket starts and the last one ends. */
  lo: number;
  hi: number;
  p50: number | null;
  p95: number | null;
  /** Where p95 falls on the axis, 0–1, for the marker. */
  p95At: number | null;
  /** Invocations with a duration — what the strip is drawn from. */
  measured: number;
  /** Invocations without one (still running, or never started). */
  unmeasured: number;
}

/**
 * Durations into `bucketCount` buckets on a log axis from the fastest to the
 * slowest. Latency is long-tailed: on a linear axis a single 30 s timeout puts
 * every 80 ms call into the first bucket and the strip says nothing.
 */
export function latencyDistribution(
  invocations: readonly Pick<FunctionInvocation, "duration_ms">[],
  bucketCount = 24
): LatencyDistribution {
  const durations = invocations
    .map((iv) => iv.duration_ms)
    .filter((ms): ms is number => ms != null && Number.isFinite(ms) && ms >= 0);
  const unmeasured = invocations.length - durations.length;
  if (durations.length === 0) {
    return {
      buckets: [],
      lo: 0,
      hi: 0,
      p50: null,
      p95: null,
      p95At: null,
      measured: 0,
      unmeasured
    };
  }
  // A 0 ms duration has no logarithm; the axis starts at 1 ms and such a call
  // lands in the first bucket.
  const lo = Math.max(1, Math.min(...durations));
  const hi = Math.max(lo * 2, Math.max(...durations));
  const span = Math.log(hi / lo);
  const at = (ms: number) => Math.min(1, Math.max(0, Math.log(Math.max(ms, lo) / lo) / span));
  const edge = (i: number) => lo * Math.exp((span * i) / bucketCount);
  const buckets: LatencyBucket[] = Array.from({ length: bucketCount }, (_, i) => ({
    from: edge(i),
    to: edge(i + 1),
    count: 0
  }));
  for (const ms of durations) {
    const i = Math.min(bucketCount - 1, Math.floor(at(ms) * bucketCount));
    buckets[i].count += 1;
  }
  const p95 = percentile(durations, 95);
  return {
    buckets,
    lo,
    hi,
    p50: percentile(durations, 50),
    p95,
    p95At: p95 == null ? null : at(p95),
    measured: durations.length,
    unmeasured
  };
}

/** How many invocations one window held, and how many of them failed. */
export interface PeriodCount {
  total: number;
  failed: number;
  /** The server sent no count and the page hit its cap: `total` is a floor. */
  atLeast: boolean;
}

/** A listing's count: the server's whole-window count when it sent one, else the
 *  page itself — a floor once the page is full. */
export function periodCount(list: AppInvocationList, limit: number): PeriodCount {
  if (list.window) return { total: list.window.total, failed: list.window.failed, atLeast: false };
  const rows = list.invocations;
  return {
    total: rows.length,
    failed: rows.filter((iv) => iv.failed).length,
    atLeast: rows.length >= limit
  };
}

export interface PeriodComparison {
  current: PeriodCount;
  previous: PeriodCount;
  /** Relative change in invocations, as a fraction (0.12 = 12% more); `null`
   *  when either side is a floor or there is nothing to compare against. */
  totalChange: number | null;
  failedChange: number | null;
  /** Failed ÷ total, `null` for an empty window. */
  failureRate: { current: number | null; previous: number | null };
}

/** This window against the equal one before it. */
export function comparePeriods(current: PeriodCount, previous: PeriodCount): PeriodComparison {
  const comparable = !current.atLeast && !previous.atLeast;
  return {
    current,
    previous,
    totalChange: comparable ? change(current.total, previous.total) : null,
    failedChange: comparable ? change(current.failed, previous.failed) : null,
    failureRate: { current: rate(current), previous: rate(previous) }
  };
}

/** `0` for nothing to nothing; `null` from nothing to something, which has no ratio. */
function change(now: number, before: number): number | null {
  if (before === 0) return now === 0 ? 0 : null;
  return (now - before) / before;
}

const rate = (count: PeriodCount): number | null =>
  count.total === 0 || count.atLeast ? null : count.failed / count.total;

/** The modes the explorer offers: the ones in the rows, plus the one selected
 *  (which a filtered list may no longer show beside the others), sorted. */
export function modeOptions(
  rows: readonly Pick<FunctionInvocation, "mode">[],
  selected?: string
): string[] {
  const modes = new Set(rows.map((iv) => iv.mode).filter(Boolean));
  if (selected) modes.add(selected);
  return [...modes].sort();
}

/** "812ms", "1.4s", "2m 5s" — a duration short enough for an axis label. */
export function formatLatency(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)}s`;
  const minutes = Math.floor(ms / 60_000);
  return `${minutes}m ${Math.round((ms % 60_000) / 1000)}s`;
}

/** "+12%", "−8%", "no change". */
export function formatChange(fraction: number): string {
  const pct = Math.round(fraction * 100);
  if (pct === 0) return "no change";
  return pct > 0 ? `+${pct}%` : `−${Math.abs(pct)}%`;
}
