import type { Fleet, FleetRow } from "@/services/api/adminObservability";
import { BASELINE_FLOOR_USD, type VersusPrior, versusPrior } from "./format";

/**
 * The fleet, ranked by cost to serve — the order the page is opened for.
 *
 * Pure, so the arithmetic the sentence and the list both read (rank, share,
 * movement, concentration) is decided once and tested, not recomputed per
 * component.
 */

/** One active org with its place in the fleet. */
export interface RankedOrg {
  row: FleetRow;
  /** 1-based, by cost to serve; ties break on name so the order is stable. */
  rank: number;
  /** Share of the fleet's total cost to serve; `null` when the total is zero. */
  share: number | null;
  /**
   * Places gained since the prior window (negative = fell). `"new"` when it
   * cost under a cent then; `null` when the prior window could not be read.
   */
  movement: number | "new" | null;
  /** Cost to serve against the prior window; `null` when it could not be read. */
  versus: VersusPrior | null;
}

export interface FleetSummary {
  /** Active orgs, most expensive first. */
  ranked: RankedOrg[];
  /** Orgs with no cost, no runs and no app traffic in the window. */
  idle: FleetRow[];
  /** The fleet's cost to serve over the window. */
  total: number;
  /** The same over the prior window; `null` when it could not be read. */
  priorTotal: number | null;
  /** The total against the prior total; `null` when it could not be read. */
  versus: VersusPrior | null;
  /** How many of the costliest orgs carry half of the total. */
  toHalf: number | null;
  /** …and 80% of it. */
  toEighty: number | null;
}

/**
 * Below this, "more than doubled" is noise: three cents becoming seven is not
 * a finding, and flagging it would spend the console's only alarm colour on it.
 */
export const JUMP_FLOOR_USD = 5;

/** No cost, no runs and no app traffic: nothing to rank. */
export function isIdle(r: FleetRow): boolean {
  return r.cost_to_serve_usd <= 0 && r.run_count === 0 && !((r.requests ?? 0) > 0);
}

/** Order by cost to serve, then by name, so equal orgs do not shuffle. */
function byCost(cost: (r: FleetRow) => number) {
  return (a: FleetRow, b: FleetRow) => cost(b) - cost(a) || a.org_name.localeCompare(b.org_name);
}

/** How many leading values it takes to reach `fraction` of `total`. */
export function countToReach(sortedDesc: number[], total: number, fraction: number): number | null {
  if (!(total > 0)) return null;
  let sum = 0;
  for (let i = 0; i < sortedDesc.length; i += 1) {
    sum += sortedDesc[i];
    // A hair of tolerance: floating-point sums that land exactly on the
    // threshold must count as reaching it.
    if (sum >= total * fraction - 1e-9) return i + 1;
  }
  return sortedDesc.length;
}

export function summarizeFleet(fleet: Fleet): FleetSummary {
  const active = fleet.rows.filter((r) => !isIdle(r)).sort(byCost((r) => r.cost_to_serve_usd));
  const idle = fleet.rows.filter(isIdle).sort((a, b) => a.org_name.localeCompare(b.org_name));
  const total = active.reduce((s, r) => s + r.cost_to_serve_usd, 0);

  // Prior ranks over every org that cost something then — including orgs
  // that are idle now, which is how a fall is measured rather than assumed.
  // "Something" is a cent or more: a first place held on $0.00000002 is not
  // a place to fall from.
  const priorRank = new Map<string, number>();
  if (fleet.prior_read) {
    const before = fleet.rows
      .filter((r) => (r.prior_cost_to_serve_usd ?? 0) >= BASELINE_FLOOR_USD)
      .sort(byCost((r) => r.prior_cost_to_serve_usd ?? 0));
    for (const [i, r] of before.entries()) priorRank.set(r.org_id, i + 1);
  }
  const priorTotal = fleet.prior_read
    ? fleet.rows.reduce((s, r) => s + (r.prior_cost_to_serve_usd ?? 0), 0)
    : null;

  const ranked: RankedOrg[] = active.map((row, i) => {
    const rank = i + 1;
    const before = priorRank.get(row.org_id);
    return {
      row,
      rank,
      share: total > 0 ? row.cost_to_serve_usd / total : null,
      movement: !fleet.prior_read ? null : before == null ? "new" : before - rank,
      versus: versusPrior(row.cost_to_serve_usd, row.prior_cost_to_serve_usd)
    };
  });

  const costs = active.map((r) => r.cost_to_serve_usd);
  return {
    ranked,
    idle,
    total,
    priorTotal,
    versus: versusPrior(total, priorTotal),
    toHalf: countToReach(costs, total, 0.5),
    toEighty: countToReach(costs, total, 0.8)
  };
}

/**
 * Cost more than doubled on a figure big enough to matter. Growth past
 * tenfold is a doubling too; an org that cost under a cent before is new.
 */
export function isCostJump(o: RankedOrg): boolean {
  const v = o.versus;
  const doubled = v?.kind === "from" || (v?.kind === "pct" && v.change >= 1);
  return doubled && o.row.cost_to_serve_usd >= JUMP_FLOOR_USD;
}

/** Case-insensitive match on name or slug. An empty query matches everything. */
export function matchesQuery(r: FleetRow, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return r.org_name.toLowerCase().includes(q) || r.org_slug.toLowerCase().includes(q);
}

/** Why there is no comparison. Only a failed read is a fault, so only it says so. */
export function priorNote(fleet: Fleet): string {
  const days = fleet.window_days;
  switch (fleet.prior_missing) {
    case "retention":
      return `No comparison: the ${days} days before this window are past data retention, so they would read as empty.`;
    case "incomparable":
      return `No comparison: compute is priced in this window but couldn’t be in the ${days} days before, so the two totals would count different things.`;
    default:
      return "Couldn’t read the prior window, so nothing here is compared against it.";
  }
}
