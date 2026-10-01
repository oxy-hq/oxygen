// A hard spend limit for one unit of work (one showcase capture, one case run
// of the suite). Before a model call, its worst case — every input token at the
// cache-write rate, and the full `max_tokens` of output — must fit in what is
// left; after it, the real cost is charged. Once the meter stops a call it
// stays stopped: nothing after it may spend, however small. With no meter,
// nothing is metered.

import { computeCost, hasRates } from "./pricing";

export class BudgetExceeded extends Error {}

/**
 * What one case run of the suite may spend before the meter stops it.
 *
 * A step that cannot succeed does not fail — the model keeps trying until
 * `max_steps`, and every turn re-sends the whole conversation so far, so the
 * bill grows with the square of the turns. A metric-tree bucket whose graph
 * could not render spent $11.50 that way across three cases (1.1–2.1M input
 * tokens each) before reporting the same failure a `wait_for:` gives for $0.
 *
 * $2 sits above every legitimate cold case — the dearest flow in
 * `_budgets.yml` is $1.55 for two — and below the $3.67 and $6.39 the two
 * runaway cases cost.
 */
export const DEFAULT_CASE_BUDGET_USD = 2;

/**
 * The per-case limit for this invocation: `AGENTIC_CASE_BUDGET_USD` in dollars,
 * the default when unset, and `undefined` (unmetered) for `0` or `off`. Anything
 * else is refused — a typo here must not quietly remove the limit.
 */
export function caseBudgetUsd(env: NodeJS.ProcessEnv = process.env): number | undefined {
  const raw = env.AGENTIC_CASE_BUDGET_USD?.trim().toLowerCase();
  if (!raw) return DEFAULT_CASE_BUDGET_USD;
  if (raw === "0" || raw === "off") return undefined;
  const usd = Number(raw);
  if (!(usd > 0)) {
    throw new Error(
      `AGENTIC_CASE_BUDGET_USD must be a dollar amount, or 0/off for no limit — got '${raw}'`
    );
  }
  return usd;
}

export interface CostMeter {
  readonly limitUsd: number;
  spentUsd: number;
  /** Set when the meter stopped a call — the run ended on budget, not on a fault. */
  stopped?: string;
}

function stop(meter: CostMeter, message: string): never {
  meter.stopped ??= message;
  throw new BudgetExceeded(meter.stopped);
}

export function createMeter(limitUsd: number): CostMeter {
  if (!(limitUsd > 0)) throw new Error(`a budget must be positive, got ${limitUsd}`);
  return { limitUsd, spentUsd: 0 };
}

export function remaining(meter: CostMeter): number {
  return Math.max(0, meter.limitUsd - meter.spentUsd);
}

/** Record spend; throws once the total is over the limit. */
export function charge(meter: CostMeter | undefined, usd: number): void {
  if (!meter) return;
  meter.spentUsd += usd;
  if (meter.spentUsd > meter.limitUsd) {
    stop(
      meter,
      `stopped at the $${meter.limitUsd.toFixed(2)} budget ($${meter.spentUsd.toFixed(3)} spent)`
    );
  }
}

/**
 * The most a call can cost: `inputBytes` of UTF-8 priced as if every two bytes
 * were a cache-written token, plus `maxOutputTokens` of output — adaptive
 * thinking counts against it too. A token of code or prose is ~4 bytes, so this
 * is about twice the real count, which also covers a tokenizer that counts up to
 * 1.35× more and most multi-byte text. It is an estimate, not a proof — a run of
 * glyphs that each split into more tokens than half their bytes could exceed it
 * — and `charge` bounds any such miss to that one call, where the meter stops
 * the run.
 */
export function worstCaseUsd(model: string, inputBytes: number, maxOutputTokens: number): number {
  return computeCost(model, {
    input: 0,
    cached_input: 0,
    cache_creation: Math.ceil(inputBytes / 2),
    output: maxOutputTokens
  });
}

/** UTF-8 size of what a call sends, for `worstCaseUsd`. */
export function bytesOf(...parts: string[]): number {
  return parts.reduce((n, p) => n + Buffer.byteLength(p, "utf8"), 0);
}

/** Throws unless the meter is still running and a call costing up to `usd` fits what is left. */
export function reserve(meter: CostMeter | undefined, usd: number): void {
  if (!meter) return;
  if (meter.stopped) stop(meter, meter.stopped);
  const left = remaining(meter);
  if (usd > left) {
    stop(
      meter,
      `stopped at the $${meter.limitUsd.toFixed(2)} budget: the next call could cost ` +
        `$${usd.toFixed(3)} and $${left.toFixed(3)} is left`
    );
  }
}

/**
 * A meter that cannot price a model would charge it $0 and never trip.
 * Metered work refuses such a model up front instead.
 */
export function ensurePriced(meter: CostMeter | undefined, model: string): void {
  if (meter && !hasRates(model)) {
    stop(meter, `no price for '${model}' in pricing.ts — a metered run cannot use it`);
  }
}
