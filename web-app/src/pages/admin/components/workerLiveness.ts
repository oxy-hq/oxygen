import type { AdminTone } from "./adminTone";

/**
 * How long a worker may go without claiming before it stops counting as alive.
 *
 * These thresholds decide whether the console says the fleet is fine, so they cannot sit
 * in one page's private helper while another page re-decides the same question with its
 * own numbers — the home and the Internal jobs page would then disagree about whether
 * anything is wrong.
 */
const ACTIVE_WITHIN_SECS = 60;
const IDLE_WITHIN_SECS = 300;

export type WorkerLiveness = "active" | "idle" | "stale";

/** Classify a worker by the age of its last claim. `null` has never claimed: stale. */
export function workerLiveness(lastClaimAt: string | null): WorkerLiveness {
  if (!lastClaimAt) return "stale";
  const ageSecs = (Date.now() - new Date(lastClaimAt).getTime()) / 1_000;
  if (ageSecs < ACTIVE_WITHIN_SECS) return "active";
  if (ageSecs < IDLE_WITHIN_SECS) return "idle";
  return "stale";
}

export const WORKER_LIVENESS_LABEL: Record<WorkerLiveness, string> = {
  active: "Active",
  idle: "Idle",
  stale: "Stale"
};

/**
 * An idle worker is `warn`, not `danger`: a quiet queue produces idle workers and
 * nothing is wrong. Only a stale one has stopped answering.
 */
export const WORKER_LIVENESS_TONE: Record<WorkerLiveness, AdminTone> = {
  active: "ok",
  idle: "warn",
  stale: "danger"
};
