import type { Fleet, FleetMedians, FleetRow } from "@/services/api/adminObservability";

/**
 * The axes an org is compared on, and how one org reads against the rest.
 *
 * Pure, because the two judgments here are worth testing without a DOM:
 *
 * 1. **A dimension nothing can answer is dropped**, not shown as a column of
 *    dashes — the same rule `fleetColumns` applies to the table.
 * 2. **An index needs a non-zero median.** Everything is infinitely more than
 *    nothing, so a zero median yields no index and the view says there is no
 *    typical account rather than printing `∞×`.
 */
export type ScaleDimensionId =
  | "cost_usd"
  | "run_count"
  | "requests"
  | "function_compute_gbs"
  | "egress_gb"
  | "storage_gib_month";

export interface ScaleDimension {
  id: ScaleDimensionId;
  label: string;
  unit: string;
  /** What the figure rests on, where that is not obvious. */
  hint?: string;
}

/**
 * Ordered by how directly each answers "what does this org take from us":
 * money first, then the work behind it, then the resources it holds.
 */
export const SCALE_DIMENSIONS: ScaleDimension[] = [
  { id: "cost_usd", label: "LLM spend", unit: "$" },
  { id: "run_count", label: "Runs", unit: "runs" },
  { id: "requests", label: "App requests", unit: "req" },
  {
    id: "function_compute_gbs",
    label: "Compute",
    unit: "GB·s",
    hint: "Provisioned, not consumed: wall time at the isolate ceiling"
  },
  { id: "egress_gb", label: "Egress", unit: "GB" },
  {
    id: "storage_gib_month",
    label: "Storage",
    unit: "GiB-mo",
    hint: "Time-weighted over the window"
  }
];

/** One org's reading on one axis. */
export interface ScaleCell {
  /** `null` when this org could not be measured on this axis. */
  value: number | null;
  /** Multiple of the fleet median; `null` when there is no usable median. */
  index: number | null;
}

export function valueOf(row: FleetRow, id: ScaleDimensionId): number | null {
  switch (id) {
    case "cost_usd":
      return row.cost_usd;
    case "run_count":
      return row.run_count;
    case "requests":
      return row.requests;
    case "function_compute_gbs":
      return row.function_compute_gbs;
    case "egress_gb":
      return row.egress_gb;
    case "storage_gib_month":
      return row.storage_gib_month;
  }
}

function medianOf(medians: FleetMedians, id: ScaleDimensionId): number | null {
  return medians[id];
}

/**
 * A value as a multiple of the median.
 *
 * Mirrors the server's `scale::index_against`, and must keep agreeing with it:
 * the server supplies the median, the client does the division, and a
 * different rule on either side would make the bar and any figure drawn from
 * the same payload disagree.
 */
export function indexOf(value: number | null, median: number | null): number | null {
  if (value === null || median === null) return null;
  if (!Number.isFinite(value) || !Number.isFinite(median) || median <= 0) return null;
  return value / median;
}

export function cellOf(row: FleetRow, medians: FleetMedians, id: ScaleDimensionId): ScaleCell {
  const value = valueOf(row, id);
  return { value, index: indexOf(value, medianOf(medians, id)) };
}

/**
 * The dimensions worth drawing: those at least one org can answer.
 *
 * A deployment with no observability store can answer spend and runs and
 * nothing else, and three empty columns would say less than their absence.
 * What is dropped is named once by the caller, as the table does.
 */
export function shownDimensions(fleet: Pick<Fleet, "rows">): {
  shown: ScaleDimension[];
  hidden: ScaleDimension[];
} {
  const shown: ScaleDimension[] = [];
  const hidden: ScaleDimension[] = [];
  for (const d of SCALE_DIMENSIONS) {
    const answerable = fleet.rows.some((r) => valueOf(r, d.id) !== null);
    (answerable ? shown : hidden).push(d);
  }
  return { shown, hidden };
}

/**
 * The widest index on any axis, for scaling every bar against one baseline.
 *
 * One baseline across the whole grid is what makes it readable down a column
 * *and* across a row: per-row scaling would make every org's largest axis look
 * identical, which is exactly the shape the view exists to show.
 *
 * Floored at 1 so a fleet where nobody exceeds the median still draws bars
 * rather than collapsing every one to full width.
 */
export function maxIndex(
  rows: FleetRow[],
  medians: FleetMedians,
  dimensions: ScaleDimension[]
): number {
  let max = 1;
  for (const r of rows) {
    for (const d of dimensions) {
      const i = cellOf(r, medians, d.id).index;
      if (i !== null && Number.isFinite(i) && i > max) max = i;
    }
  }
  return max;
}
