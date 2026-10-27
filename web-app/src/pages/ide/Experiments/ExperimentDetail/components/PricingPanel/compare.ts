import { formatGroupDay, formatUnitDay } from "@/pages/ide/Experiments/format";
import type { Grain } from "@/pages/ide/Experiments/grain";
import type { Candidate, Proposal } from "@/types/experiment";

/** Shown wherever a group design's per-unit-day figure is. */
export const GROUP_CAVEAT =
  "Per unit-day, a group design's MDE is the group MDE divided by the mean group size. It assumes the effect spreads evenly over a group's members.";

export interface RankedCandidate {
  proposal: Proposal;
  /** 1-based by ascending `mde_unit_day`; null when the candidate has no comparable MDE. */
  rank: number | null;
}

const isComparable = (p: Proposal) =>
  p.status === "done" && p.mde_unit_day !== null && Number.isFinite(p.mde_unit_day);

/**
 * Priced candidates by ascending `mde_unit_day` — the smallest effect each design
 * could read, on the one scale the platform makes comparable (index C2b). Ties
 * keep request order. Everything else follows, unranked, in request order. The
 * raw `mde` is never the sort key: a group design's is per group-day.
 */
export function rankCandidates(proposals: Proposal[]): RankedCandidate[] {
  const ranked = proposals
    .filter(isComparable)
    .sort((a, b) => (a.mde_unit_day ?? 0) - (b.mde_unit_day ?? 0));
  const rest = proposals.filter((p) => !isComparable(p));
  return [
    ...ranked.map((proposal, i) => ({ proposal, rank: i + 1 })),
    ...rest.map((proposal) => ({ proposal, rank: null }))
  ];
}

/** A group design assigns the values of a dimension, so its sample is the group. */
export function isGroupDesign(p: Proposal): boolean {
  return p.assign_by !== null;
}

/** A $ figure in the design's own unit: per group-day under `assign_by`, else per unit-day. */
export function formatOwn(p: Proposal, value: number | null, grain: Grain = "day"): string {
  return isGroupDesign(p) ? formatGroupDay(value, grain) : formatUnitDay(value, grain);
}

export const formatOwnMde = (p: Proposal, grain: Grain = "day") => formatOwn(p, p.mde, grain);

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** One line naming a candidate by what was asked for. The α is shown beside it, not in it. */
export function describeCandidate(c: Candidate, grain: Grain = "day"): string {
  const count = (n: number) => plural(n, grain, `${grain}s`);
  if (c.shape === "switchback") {
    return `Switchback ${plural(c.pairs, "pair", "pairs")} × ${count(c.period_days)} from ${c.first_period_on}`;
  }
  const parts = [`Waves ${c.wave_sizes.join(" + ")}`, `first ${c.first_switch_on}`];
  // One wave has no spacing to speak of.
  if (c.wave_sizes.length > 1) parts.push(`every ${count(c.spacing_days)}`);
  if (c.blocks) parts.push(`${c.blocks} blocks`);
  if (c.assign_by) parts.push(`by ${c.assign_by}`);
  return parts.join(", ");
}
