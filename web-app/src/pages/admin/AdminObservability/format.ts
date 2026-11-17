/**
 * How Admin → Observability renders its numbers.
 *
 * One rule runs through all of it: **`null` is not zero.** The API returns
 * `null` for a ratio over an empty window and for an app meter this deployment
 * cannot answer, and every formatter here turns that into `—` rather than a
 * zero. A page that renders "0%" for "we did not measure this" is the defect
 * the admin console's own conventions keep naming.
 */

/** Compact USD: $0.0042 · $3.21 · $1.2k · $4.5M. Cost spans many orders of
 *  magnitude here, so precision follows scale. `—` for nullish. */
export function usd(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "—";
  if (n === 0) return "$0";
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
