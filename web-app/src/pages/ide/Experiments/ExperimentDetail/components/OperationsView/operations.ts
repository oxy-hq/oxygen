import type { FlipCounts, RunPeriod, UnitGuardrail } from "@/types/experiment";

export type PeriodStatus = "upcoming" | "in_progress" | "landed" | "needs_attention";

/**
 * Flips a period owes: a cancelled flip is no longer owed. `sent` overlaps the
 * settled buckets (every flip that went out), so only the dispatched-not-settled
 * remainder is added to them.
 */
export function flipTotal(f: FlipCounts): number {
  const settled = f.confirmed + f.unconfirmed + f.failed;
  const dispatched = Math.max(0, f.sent - settled);
  return f.pending + settled + dispatched;
}

/**
 * Where a period is. `landed` only when every flip owed is confirmed; any failed
 * or unconfirmed flip needs attention even if the rest landed — the report would
 * otherwise measure the attempt in that period without saying so.
 */
export function periodStatus(f: FlipCounts): PeriodStatus {
  const total = flipTotal(f);
  if (total === 0) return "upcoming";
  if (f.failed > 0 || f.unconfirmed > 0) return "needs_attention";
  return f.confirmed === total ? "landed" : "in_progress";
}

/** Periods come in consecutive pairs of one on and one off: periods 0 and 1 are pair 1. */
export const pairOf = (periodIndex: number): number => Math.floor(periodIndex / 2) + 1;

export interface PeriodRow {
  index: number;
  pair: number;
  from: string;
  state: "on" | "off";
  confirmed: number;
  total: number;
  failed: number;
  status: PeriodStatus;
}

export function periodRows(periods: RunPeriod[]): PeriodRow[] {
  return periods.map((p) => ({
    index: p.index,
    pair: pairOf(p.index),
    from: p.from,
    state: p.state,
    confirmed: p.flips.confirmed,
    total: flipTotal(p.flips),
    failed: p.flips.failed,
    status: periodStatus(p.flips)
  }));
}

export interface GroupRow {
  group: string;
  members: string[];
}

/** Pinned groups by name. `null` (not pinned yet) is no rows, not an empty design. */
export function groupRows(membership: Record<string, string[]> | null): GroupRow[] {
  if (membership === null) return [];
  return Object.entries(membership)
    .map(([group, members]) => ({ group, members }))
    .sort((a, b) => a.group.localeCompare(b.group));
}

export function membershipTotals(rows: GroupRow[]): { groups: number; members: number } {
  return { groups: rows.length, members: rows.reduce((n, r) => n + r.members.length, 0) };
}

export interface UnitGuardrailSummary {
  breached: UnitGuardrail[];
  /** Refused or not checked yet: the guardrail said nothing about these. */
  unevaluated: UnitGuardrail[];
  ok: number;
}

/** A 200-store chain is not 200 rows: name the units that matter and count the rest. */
export function summariseUnits(units: UnitGuardrail[]): UnitGuardrailSummary {
  return {
    breached: units.filter((u) => u.status === "breached"),
    unevaluated: units.filter((u) => u.status === "refused" || u.status === "not_checked"),
    ok: units.filter((u) => u.status === "ok").length
  };
}
