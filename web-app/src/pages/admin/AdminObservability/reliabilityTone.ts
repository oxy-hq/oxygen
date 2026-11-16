import type { AdminTone } from "../components/adminTone";

/**
 * What tone a failure rate earns.
 *
 * On this surface **colour means something is wrong**, so a healthy rate takes
 * no colour at all — it stays in the body foreground, which is what `muted`
 * resolves to here. Only a rate worth looking at gets one, and it maps onto an
 * existing tone rather than inventing a sixth.
 *
 * The thresholds are the SLO the custom-app burn-rate evaluator already works
 * against: an app is allowed about 1% failures, so 1% is where a rate stops
 * being noise, and 5× the allowance is where it stops being survivable. They
 * are stated here, once, rather than spelled into two tables.
 *
 * `null` — no traffic, or no observability store — is `muted` and renders as a
 * dash. An org with no requests has **no opinion** about reliability, and
 * drawing it green says the opposite.
 */
export const RELIABILITY_WARN_AT = 0.01;
export const RELIABILITY_DANGER_AT = 0.05;

export function reliabilityTone(failureRate: number | null | undefined): AdminTone {
  if (failureRate == null || !Number.isFinite(failureRate)) return "muted";
  if (failureRate >= RELIABILITY_DANGER_AT) return "danger";
  if (failureRate >= RELIABILITY_WARN_AT) return "warn";
  return "muted";
}

/** Does this rate deserve a colour at all? */
export function isReliabilityConcern(failureRate: number | null | undefined): boolean {
  return reliabilityTone(failureRate) !== "muted";
}
