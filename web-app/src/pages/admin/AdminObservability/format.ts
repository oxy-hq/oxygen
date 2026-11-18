/**
 * How Admin → Observability renders its numbers.
 *
 * One rule runs through all of it: **`null` is not zero.** The API returns
 * `null` for a ratio over an empty window and for an app meter this deployment
 * cannot answer, and every formatter here turns that into `—` rather than a
 * zero. A page that renders "0%" for "we did not measure this" is the defect
 * the admin console's own conventions keep naming.
 */

/**
 * The smallest amount four decimals can write. A few bytes of storage cost
 * $0.00000002 a month, which `toFixed(4)` prints as "$0.0000": a cost that was
 * not zero, drawn as one. Below this, `usd` says how small instead.
 */
const SMALLEST_WRITTEN_USD = 0.0001;

/** Compact USD: <$0.0001 · $0.0042 · $3.21 · $1.2k · $4.5M. Cost spans many
 *  orders of magnitude here, so precision follows scale. `—` for nullish. */
export function usd(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "—";
  if (n === 0) return "$0";
  if (n > 0 && n < SMALLEST_WRITTEN_USD) return "<$0.0001";
  if (n < 0.01) return `$${n.toFixed(4)}`;
  if (n < 1_000) return `$${n.toFixed(2)}`;
  if (n < 1_000_000) return `$${(n / 1_000).toFixed(1)}k`;
  return `$${(n / 1_000_000).toFixed(2)}M`;
}

/** Compact counts: 1,204 · 12.0k · 3.4M · 1.10B. `—` for nullish. */
export function compact(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "—";
  if (n < 1_000) return `${Math.round(n)}`;
  if (n < 1_000_000) return `${(n / 1_000).toFixed(1)}k`;
  if (n < 1_000_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  return `${(n / 1_000_000_000).toFixed(2)}B`;
}

/**
 * A duration in milliseconds, at the precision the number deserves.
 *
 * Sub-second latency is read in whole milliseconds; past a second the
 * milliseconds stop mattering and the seconds start to. `0` is a real answer
 * — a cached response really can land in under a millisecond — so it renders
 * as `0 ms`, not as `—`.
 */
export function ms(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "—";
  if (n < 1_000) return `${Math.round(n)} ms`;
  if (n < 60_000) return `${(n / 1_000).toFixed(1)} s`;
  return `${Math.round(n / 1_000)} s`;
}

/**
 * A fraction as a percentage. `—` for nullish, which is what an empty window
 * and an unmeasured meter both produce.
 *
 * Small non-zero rates keep a decimal: a 0.4% failure rate rounded to "0%"
 * reads as perfect, and the difference between none and nearly-none is the
 * whole signal on an error-rate column.
 */
export function pct(fraction: number | null | undefined, digits = 0): string {
  if (fraction == null || !Number.isFinite(fraction)) return "—";
  const v = fraction * 100;
  if (v > 0 && v < 1) return `${v.toFixed(1)}%`;
  return `${v.toFixed(digits)}%`;
}

/** Bytes, binary, at one decimal past kilobytes. `—` for nullish. */
export function bytes(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "—";
  if (n < 1024) return `${Math.round(n)} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

/** Short model label — drops the vendor prefix and the date suffix. */
export function shortModel(model: string): string {
  return model
    .replace(/^(us\.)?(anthropic\.)?/, "")
    .replace(/-\d{8}$/, "")
    .replace(/-latest$/, "");
}

/**
 * What a `source_type` is called on screen.
 *
 * The stored values are wire contracts — `workflow` is what an automation's
 * runs have been stamped with since before the rename, and `analytics` is
 * what a chat run carries. Renaming them in the database was ruled out (they
 * are storage contracts); renaming them *here* is the whole job of a label.
 */
const SURFACE_LABELS: Record<string, string> = {
  analytics: "Chat",
  workflow: "Automations",
  airway: "Airway ELT",
  builder: "Builder"
};

export function surfaceLabel(key: string): string {
  return SURFACE_LABELS[key] ?? key;
}

/**
 * What a failure `outcome` is called on screen, and why it matters.
 *
 * These are the three SRE classes `custom_app_events.outcome` exists to keep
 * apart, and the names are chosen so an operator can tell them apart at a
 * glance: an explicit failure is a 5xx, an implicit one is a 200 that carried
 * a shell which never mounted, and a policy one is a 200 that took too long.
 */
const OUTCOME_LABELS: Record<string, string> = {
  error: "Server error",
  empty: "Never mounted",
  slow: "Too slow",
  timeout: "Timed out"
};

export function outcomeLabel(outcome: string): string {
  return OUTCOME_LABELS[outcome] ?? outcome;
}

/**
 * A change, signed with a real minus: `+12%`, `−8%`. Under half a percent
 * reads `±0%` rather than a sign that would claim a direction.
 */
export function signedPct(change: number | null | undefined): string {
  if (change == null || !Number.isFinite(change)) return "—";
  if (Math.abs(change) < 0.005) return "±0%";
  return `${change > 0 ? "+" : "−"}${pct(Math.abs(change))}`;
}

/**
 * Dollars for a ranked figure: whole dollars from $10 up (`$1,842`, `$669`),
 * cents below that. Where `usd` compacts to `$1.8k` for dense tables, a
 * ranking compares neighbours and needs the digits.
 */
export function money(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "—";
  if (n === 0) return "$0";
  if (n >= 10) return `$${Math.round(n).toLocaleString("en-US")}`;
  if (n >= 0.01) return `$${n.toFixed(2)}`;
  return usd(n);
}

/**
 * Below a cent, a prior window is no baseline. Dev's fleet once cost
 * $0.00000002 of storage in a month and $47 the next, which divided through
 * reads "up 231432043622%": true arithmetic, and a number nothing on the page
 * can be traced to. Every money figure here rounds to the cent, so a cent is
 * the smallest prior a reader can check a percentage against.
 */
export const BASELINE_FLOOR_USD = 0.01;

/** At ten times the baseline, "+900%" stops reading as a size; the baseline itself does. */
const TENFOLD = 10;

/** How a cost stands against the same span before it. */
export type VersusPrior =
  /** The prior cost under a cent (or nothing): no baseline to compare with. */
  | { kind: "new"; prior: number }
  /** A percentage a reader can check: `change` is fractional (0.23 = +23%). */
  | { kind: "pct"; change: number }
  /** Ten times the prior or more: say where it grew from, not a four-digit percentage. */
  | { kind: "from"; prior: number };

/** `null` when the prior window was not read — no comparison, not a zero one. */
export function versusPrior(current: number, prior: number | null | undefined): VersusPrior | null {
  if (prior == null || !Number.isFinite(prior)) return null;
  if (prior < BASELINE_FLOOR_USD) return { kind: "new", prior };
  if (current >= prior * TENFOLD) return { kind: "from", prior };
  return { kind: "pct", change: current / prior - 1 };
}

/** For a cell or a tooltip: `+23%`, `−8%`, `±0%`, `from $0.42`, `new`. */
export function versusShort(v: VersusPrior): string {
  switch (v.kind) {
    case "new":
      return "new";
    case "pct":
      return signedPct(v.change);
    case "from":
      return `from ${money(v.prior)}`;
  }
}

/** A prior cost as a sentence reads it: "against nothing", "against under $0.01". */
export function priorAmount(prior: number): string {
  if (prior <= 0) return "nothing";
  if (prior < BASELINE_FLOOR_USD) return "under $0.01";
  return money(prior);
}
