// Anthropic pricing for the models the agentic runner uses. Hermetic — no
// network call. Update PRICING_AS_OF and the rate table below when Anthropic
// changes pricing; the date is printed in every run's output so historical
// results can be re-priced if needed.
//
// Source: https://www.anthropic.com/pricing
//
// Conventions used by Anthropic billing:
//   - Cache write tokens are charged at 1.25× the base input rate.
//   - Cache read (hit) tokens are charged at 0.10× the base input rate.
//   - Output tokens have their own rate.
//
// All rates below are USD per 1M tokens.

import type { TokenUsage } from "./types";

export const PRICING_AS_OF = "2026-05-04";

interface ModelRates {
  /** $/1M uncached input tokens. */
  input: number;
  /** $/1M cache-write tokens (typically 1.25× input). */
  cache_write: number;
  /** $/1M cache-read tokens (typically 0.10× input). */
  cache_read: number;
  /** $/1M output tokens. */
  output: number;
}

const RATES: Record<string, ModelRates> = {
  // Claude Opus 5 — $5 / $25 (the showcase planner can be pointed at it)
  "claude-opus-5": { input: 5, cache_write: 6.25, cache_read: 0.5, output: 25 },
  // Claude Sonnet 4.x family — $3 / $15
  "claude-sonnet-4-7": { input: 3, cache_write: 3.75, cache_read: 0.3, output: 15 },
  "claude-sonnet-4-6": { input: 3, cache_write: 3.75, cache_read: 0.3, output: 15 },
  "claude-sonnet-4-5": { input: 3, cache_write: 3.75, cache_read: 0.3, output: 15 },
  // Computer-use Sonnet 4 (Stagehand v3 CUA default) — same Sonnet pricing
  "claude-sonnet-4-20250514": { input: 3, cache_write: 3.75, cache_read: 0.3, output: 15 },
  // Claude Haiku 4.5 — $1 / $5
  "claude-haiku-4-5": { input: 1, cache_write: 1.25, cache_read: 0.1, output: 5 },
  "claude-haiku-4-5-20251001": { input: 1, cache_write: 1.25, cache_read: 0.1, output: 5 }
};

const warned = new Set<string>();

/** Returns the rates for a model, or null if unknown (and warns once). */
function getRates(model: string): ModelRates | null {
  const direct = RATES[model];
  if (direct) return direct;
  // Strip a trailing -YYYYMMDD date suffix so versioned aliases price correctly.
  const stripped = model.replace(/-\d{8}$/, "");
  const fallback = RATES[stripped];
  if (fallback) return fallback;
  if (!warned.has(model)) {
    warned.add(model);
    console.warn(
      `[pricing] unknown model '${model}' — cost will be reported as 0. ` +
        `Update web-app/tests/agentic/runner/pricing.ts to add it.`
    );
  }
  return null;
}

/** True when `computeCost` can price the model (a meter must never see a silent $0). */
export function hasRates(model: string): boolean {
  return Boolean(RATES[model] ?? RATES[model.replace(/-\d{8}$/, "")]);
}

/** Compute USD cost for a token usage record under a given model. */
export function computeCost(model: string, tokens: TokenUsage): number {
  const r = getRates(model);
  if (!r) return 0;
  return (
    (tokens.input * r.input) / 1_000_000 +
    (tokens.cached_input * r.cache_read) / 1_000_000 +
    (tokens.cache_creation * r.cache_write) / 1_000_000 +
    (tokens.output * r.output) / 1_000_000
  );
}

/** Format a USD value with 4 decimal places, prefixed with $. */
export function fmtUsd(usd: number): string {
  return `$${usd.toFixed(4)}`;
}
