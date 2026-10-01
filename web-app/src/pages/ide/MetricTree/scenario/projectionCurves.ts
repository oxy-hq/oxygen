import type { ForecastPoint, ImpactConfidence, MeasureProjection } from "@/types/metricTree";

/**
 * Compose the scenario curve from the baseline curve and the propagated impact.
 *
 * Pure, and deliberately so: the server returns the BASELINE curve, which is a
 * warehouse query keyed on (levers, period, scope, granularity, horizon), while
 * the lever's value changes on every keystroke. Same split as
 * `baseline` / `predict`, one layer up.
 *
 * ## The rule
 *
 * A lever's move is read as a **uniform proportional shift** — the same reading
 * `propagate_delta` gives it, where `r = Δchild ÷ child`. So
 *
 * ```
 * scenario(bucket) = baseline(bucket) × (1 + Δ ÷ window_total)
 * ```
 *
 * and `window_total` is the measure's baseline aggregate — the very number
 * `predict` sized Δ against, so the two cannot disagree about what the
 * proportion is relative to.
 *
 * One rule, not two, and that is the point of writing it this way. For an
 * additive measure (a sum over the window) scaling every bucket by `1 + r`
 * moves the window total by exactly Δ. For a ratio (`prime_cost_pct`, a wage,
 * a per-check average) the window aggregate is not the sum of its buckets, and
 * the *other* obvious reading — spreading Δ ÷ n over the buckets — is nonsense
 * there. The proportional form is correct for both, so nothing here has to ask
 * which kind of measure it is holding, and there is no branch to get wrong.
 *
 * ## What it refuses
 *
 * Every refusal below is a case where a curve could be drawn and would be
 * wrong. In particular an `unquantifiable` impact carries `estimated_delta: 0`
 * meaning *unknown* — shifting by zero draws the scenario exactly on top of the
 * baseline, which reads as "this lever changes nothing". That is the single
 * most dangerous line this file could draw, so it is the first case checked.
 */

/** Why a measure has no scenario curve. Each names a different fix. */
export type ScenarioRefusal =
  /** The model knows the measure moves but cannot size it. NOT zero. */
  | "unquantifiable"
  /** No forward baseline to shift — the series refused its own fit. */
  | "no_forecast"
  /** No window aggregate, so no proportion to shift by. */
  | "no_baseline_value"
  /** The window aggregate is 0: a proportional change of zero is undefined. */
  | "zero_baseline"
  /** Nothing propagated here — the lever sits at its current value, or this
   *  measure is simply not downstream of it. */
  | "unmoved"
  /** The effect lands after the last projected bucket. Two identical curves
   *  would say "this lever changes nothing over the horizon"; it changes
   *  nothing *yet*, which is a different and useful statement. */
  | "lands_after_horizon";

interface ScenarioPoint {
  date: string;
  value: number;
}

export type ScenarioCurve =
  | {
      kind: "curve";
      points: ScenarioPoint[];
      /** Bucket the effect first lands in — where the two curves separate. */
      landsAt: string;
      /**
       * From the impact. Grades the propagation *edge*, not the forecast —
       * `ConfidenceMark` shows the distinction; the chart does not vary stroke
       * by it, since a scenario curve is the same forecast times a constant.
       */
      confidence: ImpactConfidence;
    }
  | { kind: "refused"; reason: ScenarioRefusal };

export interface ScenarioCurveInput {
  projection: MeasureProjection;
  /** The measure's baseline window aggregate, from `BaselineResponse.values`. */
  baselineValue: number | undefined;
  /** The propagated delta — `PredictImpact.estimated_delta`, or a lever's own
   *  resolved delta. `undefined` when nothing reached this measure. */
  delta: number | undefined;
  confidence: ImpactConfidence | undefined;
  /** Accumulated lag in days along the propagation path. `null`/`undefined`
   *  reads as 0 — a lever's own move lands immediately, and so does an impact
   *  no edge on whose path declares a lag. */
  lagDays?: number | null;
}

export function scenarioCurve(input: ScenarioCurveInput): ScenarioCurve {
  const { projection, baselineValue, delta, confidence } = input;

  // First, and unconditionally: `unquantifiable` arrives with a delta of 0
  // that means "unknown". Falling through would draw the scenario on top of
  // the baseline — the surface stating "no change" where the model stated
  // "cannot size".
  if (confidence === "unquantifiable") return { kind: "refused", reason: "unquantifiable" };
  if (delta === undefined || !Number.isFinite(delta)) {
    return { kind: "refused", reason: "unmoved" };
  }
  if (projection.forecast.length === 0) return { kind: "refused", reason: "no_forecast" };
  if (baselineValue === undefined || !Number.isFinite(baselineValue)) {
    return { kind: "refused", reason: "no_baseline_value" };
  }
  if (baselineValue === 0) return { kind: "refused", reason: "zero_baseline" };

  const ratio = 1 + delta / baselineValue;
  const landsAt = landingBucket(projection.forecast, input.lagDays ?? 0);
  if (landsAt === null) return { kind: "refused", reason: "lands_after_horizon" };

  return {
    kind: "curve",
    landsAt,
    confidence: confidence ?? "estimated",
    points: projection.forecast.map((bucket) => ({
      date: bucket.date,
      // Before the effect lands the two curves are the same line, which is the
      // whole visible content of `lag`: the analyst sees three weeks of nothing
      // before revenue moves.
      value: bucket.date < landsAt ? bucket.point : bucket.point * ratio
    }))
  };
}

/**
 * The bucket the effect lands in: the last one starting on or before
 * `lagDays` past the horizon's start. `null` when that date is past the
 * horizon entirely.
 *
 * Days against calendar bucket starts — a lag is stated in days whatever the
 * bucket width, so a 21-day lag on weekly buckets lands in the fourth one, not
 * the twenty-second. And it is the last bucket starting *before* the landing
 * date, not the first starting after: a bucket is labelled by its start, so a
 * mid-week landing belongs to the week already running. Rounding it up would
 * push every non-daily effect a whole bucket late.
 */
function landingBucket(forecast: ForecastPoint[], lagDays: number): string | null {
  const first = forecast[0];
  if (!first) return null;
  const landing = addDays(first.date, Math.max(0, Math.round(lagDays)));

  const beyond = forecast.findIndex((bucket) => bucket.date > landing);
  const index = beyond === -1 ? forecast.length - 1 : beyond - 1;
  const landed = forecast[index];
  if (!landed) return first.date; // lag ≤ 0: lands immediately.
  if (index < forecast.length - 1) return landed.date;

  // It lands at or after the LAST bucket's start, so it is inside the horizon
  // only if that bucket is still running when it lands. Bucket width comes
  // from the gap between the final two; with a single bucket there is no gap
  // to measure, so any positive lag reads as past the horizon — the honest
  // answer for a one-bucket projection.
  const previous = forecast[index - 1]?.date;
  const width = previous ? Math.max(1, daysBetween(previous, landed.date)) : 1;
  return landing < addDays(landed.date, width) ? landed.date : null;
}

/** `YYYY-MM-DD` + n days, in UTC. String dates so they compare lexically,
 *  which is why every comparison above can use `<` and `<=` directly. */
export function addDays(date: string, days: number): string {
  const d = new Date(`${date}T00:00:00Z`);
  d.setUTCDate(d.getUTCDate() + days);
  return d.toISOString().slice(0, 10);
}

function daysBetween(from: string, to: string): number {
  const ms = Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`);
  return Math.round(ms / 86_400_000);
}

/** One sentence per refusal, for the panel. */
export const SCENARIO_REFUSAL_TEXT: Record<ScenarioRefusal, string> = {
  unquantifiable: "can't size this move — no scenario curve",
  no_forecast: "no forecast to shift",
  no_baseline_value: "no baseline value to shift against",
  zero_baseline: "baseline is zero — a proportional change is undefined",
  unmoved: "this lever doesn't move this measure",
  lands_after_horizon: "the effect lands after this horizon"
};
