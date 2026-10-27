import type { Proposal } from "@/types/experiment";

/**
 * Test-only builders. A priced waves proposal by default; override what a test
 * cares about. Nothing in the app imports this file.
 */
export function proposal(over: Partial<Proposal> & { id: string }): Proposal {
  return {
    request_id: "req-1",
    status: "done",
    created_at: "2026-09-30T01:00:00Z",
    finished_at: "2026-09-30T01:01:00Z",
    request: {
      shape: "waves",
      wave_sizes: [3, 3],
      first_switch_on: "2099-06-01",
      spacing_days: 14,
      alpha: 0.05
    },
    seed: 8841207,
    waves: [
      { switch_on: "2099-06-01", units: ["a", "b", "c"] },
      { switch_on: "2099-06-15", units: ["d", "e", "f"] }
    ],
    periods: null,
    strata: null,
    assign_by: null,
    mde: 17.5,
    mde_relative: 0.052,
    baseline: 336.5,
    mde_unit_day: 17.5,
    alpha: 0.05,
    prior_coefficient: null,
    warnings: [],
    refusal: null,
    snippet: "design:\n  seed: 8841207\noutcome:\n  mde_at_proposal: 17.5\n",
    design_id: "design-2",
    stale: false,
    ...over
  };
}

export function pending(id: string, over: Partial<Proposal> = {}): Proposal {
  return proposal({
    id,
    status: "pending",
    finished_at: null,
    seed: null,
    waves: null,
    mde: null,
    mde_relative: null,
    baseline: null,
    mde_unit_day: null,
    snippet: null,
    ...over
  });
}

export function refused(id: string, refusal: string, over: Partial<Proposal> = {}): Proposal {
  return pending(id, { status: "refused", finished_at: "2026-09-30T01:01:00Z", refusal, ...over });
}

export function failed(id: string, reason: string, over: Partial<Proposal> = {}): Proposal {
  return pending(id, {
    status: "failed",
    finished_at: "2026-09-30T01:01:00Z",
    refusal: reason,
    ...over
  });
}

/** Four periods = two pairs, `on, off, off, on`. */
export function switchback(id: string, over: Partial<Proposal> = {}): Proposal {
  return proposal({
    id,
    request: {
      shape: "switchback",
      first_period_on: "2099-06-01",
      period_days: 7,
      pairs: 2,
      alpha: 0.05
    },
    waves: null,
    periods: [
      { from: "2099-06-01", state: "on" },
      { from: "2099-06-08", state: "off" },
      { from: "2099-06-15", state: "off" },
      { from: "2099-06-22", state: "on" }
    ],
    mde: 12,
    mde_unit_day: 12,
    snippet: "design:\n  switchback:\n    period_days: 7\n",
    ...over
  });
}

/** A grouped design: its own MDE is per GROUP-day, `mde_unit_day` divides by the mean group size. */
export function grouped(id: string, over: Partial<Proposal> = {}): Proposal {
  return proposal({
    id,
    request: {
      shape: "waves",
      wave_sizes: [2, 2],
      first_switch_on: "2099-06-01",
      spacing_days: 14,
      assign_by: "category",
      alpha: 0.05
    },
    assign_by: "category",
    waves: [
      { switch_on: "2099-06-01", units: ["Beverages", "Snacks"] },
      { switch_on: "2099-06-15", units: ["Dairy", "Frozen"] }
    ],
    mde: 1200,
    mde_unit_day: 24,
    snippet: "assign_by: category\ndesign:\n  seed: 7\n",
    ...over
  });
}
