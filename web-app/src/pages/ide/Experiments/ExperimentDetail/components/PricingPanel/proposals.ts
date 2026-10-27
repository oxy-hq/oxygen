import type { Proposal } from "@/types/experiment";

/** Pricing is a job per candidate (placebo power is minutes, not milliseconds). */
export const PRICING_TIMEOUT_MS = 180_000;

/** The request the page queued: its id, the proposal rows it created, and when. */
export interface Awaiting {
  requestId: string;
  ids: string[];
  startedAt: number;
}

export type PricingStatus = "idle" | "waiting" | "timed_out" | "settled";

export interface PricingView {
  status: PricingStatus;
  /** The request's proposals found so far, in the order they were created. */
  proposals: Proposal[];
  /** Candidates the request has (the awaited ids, or the rows found when resuming). */
  total: number;
  /** Of those, how many have left `pending`. */
  settled: number;
}

/** The newest proposal by `created_at`, whatever order the server sent. */
export function newestProposal(proposals: Proposal[]): Proposal | null {
  let newest: Proposal | null = null;
  for (const p of proposals) {
    if (newest === null || Date.parse(p.created_at) > Date.parse(newest.created_at)) newest = p;
  }
  return newest;
}

/** One request's candidates, oldest first (the order they were sent); ties by id so a re-read agrees. */
export function requestProposals(proposals: Proposal[], requestId: string): Proposal[] {
  return proposals
    .filter((p) => p.request_id === requestId)
    .sort(
      (a, b) => Date.parse(a.created_at) - Date.parse(b.created_at) || a.id.localeCompare(b.id)
    );
}

/**
 * Where pricing is, and which proposals belong to it.
 *
 * A request is several rows before it is several tasks (PR 3), so every terminal
 * outcome — `done`, `refused`, `failed` — is a proposal that leaves `pending`;
 * none of them is a timeout, and the request is settled only when EVERY one of
 * its candidates has. While a request is awaited only *its* rows count, so an
 * older result is never shown as the answer to the new question. With nothing
 * awaited the newest request is shown, and a `pending` row resumes the wait (a
 * reload mid-job, bounded by the same timeout from `resumedAt`). A request that settles after the deadline is still reported:
 * the answer beats the clock.
 */
export function pricingView(args: {
  awaiting: Awaiting | null;
  proposals: Proposal[];
  now: number;
  timeoutMs: number;
  /**
   * When the page began waiting on a request it did not queue (a reload
   * mid-job). Without it a row that never settles would be waited on forever.
   */
  resumedAt?: number;
}): PricingView {
  const { awaiting, proposals, now, timeoutMs, resumedAt } = args;
  const requestId = awaiting ? awaiting.requestId : (newestProposal(proposals)?.request_id ?? null);
  if (requestId === null) return { status: "idle", proposals: [], total: 0, settled: 0 };

  const rows = requestProposals(proposals, requestId);
  const total = awaiting ? awaiting.ids.length : rows.length;
  const settled = rows.filter((p) => p.status !== "pending").length;
  const view = { proposals: rows, total, settled };

  if (total > 0 && settled >= total) return { status: "settled", ...view };
  const waitingSince = awaiting ? awaiting.startedAt : resumedAt;
  if (waitingSince !== undefined && now - waitingSince >= timeoutMs) {
    return { status: "timed_out", ...view };
  }
  return { status: "waiting", ...view };
}

/**
 * Candidates that put units in an assignment. A refusal or a fault is logged but
 * assigns nothing; every priced candidate counts, side by side or not, because
 * choosing among draws is choosing among draws.
 */
export function assignmentsProposed(proposals: Proposal[]): number {
  return proposals.filter((p) => p.status === "done").length;
}
