// Per-node scenario render data, and the formatters that turn its numbers into
// the strings the canvas and the side panel both show. Split out of
// `ScenarioNode.tsx` so the value row, the impact list and the lever list can
// share them without importing a React component for a type.

import type { ImpactConfidence, MetricNode, UnvaluedReason } from "@/types/metricTree";

export type ScenarioNodeState =
  | "lever"
  | "impacted"
  | "unquantifiable"
  /** Reachable and valued, but the scenario moved it by nothing. Distinct
   *  from `unreachable` (the lever cannot touch it at all) and from
   *  `unquantifiable` (it moved by an amount the model cannot size). */
  | "unchanged"
  | "unvalued"
  | "unreachable";

/**
 * How each state is drawn on the canvas, and the single source of truth for
 * whether a node is in the foreground of the scenario.
 *
 * The edges read this too: an edge is only drawn at full strength when BOTH of
 * its endpoints are in the foreground. Without that, a lit edge ran into a
 * dimmed card and implied the scenario had propagated somewhere it hadn't.
 *
 * Nothing here uses the card's `blurred` treatment, and that is deliberate.
 * Receding a node must never cost legibility on THIS surface: a measure the
 * current lever cannot reach is the most likely candidate for the NEXT lever,
 * and pinning one means finding it by name and clicking it. `dimmed` drops the
 * accent and mutes the text while leaving both readable; blur made the only
 * path to a second lever a guess. (The World Model does blur, but there the
 * blurred cards are background to a breakdown, not the thing you click.)
 */
export const SCENARIO_NODE_PRESENTATION: Record<
  ScenarioNodeState,
  { selected?: boolean; dimmed?: boolean }
> = {
  lever: { selected: true },
  impacted: {},
  unquantifiable: {},
  unchanged: { dimmed: true },
  unvalued: { dimmed: true },
  unreachable: { dimmed: true }
};

/** Opacity of an edge whose endpoints are not both in the scenario's foreground. */
const RECEDED_EDGE_OPACITY = 0.15;

/** Opacity an edge is drawn at given its endpoints' states. `undefined` means
 *  the node is not in the scenario map at all, which recedes like the rest. */
export function scenarioEdgeOpacity(
  source: ScenarioNodeState | undefined,
  target: ScenarioNodeState | undefined,
  litOpacity: number
): number {
  const inForeground = (state: ScenarioNodeState | undefined) =>
    state !== undefined && !SCENARIO_NODE_PRESENTATION[state].dimmed;
  return inForeground(source) && inForeground(target) ? litOpacity : RECEDED_EDGE_OPACITY;
}

export interface ScenarioNodeData {
  node: MetricNode;
  state: ScenarioNodeState;
  baseline?: number;
  /** `baseline + delta`. Set for an impact and for a **lever** — the lever's
   *  own move is resolved client-side and never comes back in `predict`'s
   *  impacts, so omitting it here left every lever surface reporting the
   *  baseline the analyst had just moved away from. */
  simulated?: number;
  /** The change itself: propagated from `predict` for an impact, resolved from
   *  what was typed for a lever. In delta-only mode (no time dimension, hence
   *  no baseline) this is the only number available, so it is what the node
   *  renders — without it a pinned lever looks inert. */
  delta?: number;
  /** How much of a claim `simulated`/`delta` is. Absent on a lever, whose
   *  value is a given rather than a calculation. */
  confidence?: ImpactConfidence;
  unvaluedReason?: UnvaluedReason;
  /** Exactly what the analyst typed for a lever ("11", "+5%"). */
  leverRaw?: string;
  /** `[lever, …, this measure]` — the route `predict` walked to reach it.
   *
   *  The FIRST such route, not the only one: `estimated_delta` sums every path
   *  into a node while this reports one of them. Anything rendering it has to
   *  say so when more than one exists (see `countPathsTo`), or a two-route
   *  number reads as the product of the single route on screen. */
  path?: string[];
  /** Days between the change and the effect landing, accumulated over the whole
   *  path. Absent when no edge on it declares a lag. */
  lag?: number | null;
}

/** Strip trailing zeros only *after* a decimal point: `"2.10"` → `"2.1"`,
 *  `"2.00"` → `"2"`, but `"310"` stays `"310"`. Stripping unconditionally
 *  turned 310K into 31K — a silent 10× understatement. */
function trimTrailingZeros(fixed: string): string {
  return fixed.includes(".") ? fixed.replace(/\.?0+$/, "") : fixed;
}

/** Scale/suffix pairs, ascending. `B` and `T` exist so a value past a billion
 *  has a unit to be promoted into rather than four digits under `M`. */
const UNITS: readonly (readonly [number, string])[] = [
  [1, ""],
  [1_000, "K"],
  [1_000_000, "M"],
  [1_000_000_000, "B"],
  [1_000_000_000_000, "T"]
] as const;

/** `2_410_000` → `"2.41M"`. Compact by design — the node has little width. */
export function formatValue(value: number): string {
  const abs = Math.abs(value);
  let i = 0;
  while (i + 1 < UNITS.length && abs >= UNITS[i + 1][0]) i++;
  if (i === 0) return Number.isInteger(value) ? `${value}` : value.toFixed(2);

  // Three significant figures can carry a value up a whole unit — 999_999 is
  // 999.999K, which rounds to 1000K. `toPrecision(3)` won't print that fourth
  // digit; it switches to exponential the moment the exponent reaches the
  // precision, which is how `"1.00e+3K"` reached the canvas (and `"1.50e+3M"`
  // for 1.5e9). Promote to the next unit instead, which is what the value
  // actually is: 1.00M.
  if (Math.abs(Number((value / UNITS[i][0]).toPrecision(3))) >= 1_000 && i + 1 < UNITS.length) {
    i++;
  }
  const [scale, suffix] = UNITS[i];
  return `${trimTrailingZeros((value / scale).toPrecision(3))}${suffix}`;
}

/** `(2_410_000, 2_100_000)` → `"+14.8%"`. */
export function formatPercent(simulated: number, baseline: number): string {
  if (baseline === 0) return "—";
  const pct = ((simulated - baseline) / Math.abs(baseline)) * 100;
  const sign = pct >= 0 ? "+" : "";
  return `${sign}${pct.toFixed(1)}%`;
}
