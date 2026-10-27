import type { Grain } from "./grain";

/** What a missing or non-finite number renders as. Never `0`. */
export const NOT_AVAILABLE = "—";

const TWO_DECIMALS = new Intl.NumberFormat("en-US", {
  minimumFractionDigits: 2,
  maximumFractionDigits: 2
});

const isNumber = (value: number | null): value is number =>
  value !== null && Number.isFinite(value);

/**
 * `17.50/unit-day`, `-3.10/unit-week`; a missing number is a dash. The period is
 * the design's grain (`design.grain`), day unless given. No currency: the outcome
 * is the target measure's own unit (dollars, units sold, hours), and nothing in the
 * design or the semantic model says which.
 */
export function formatUnitDay(value: number | null, grain: Grain = "day"): string {
  if (!isNumber(value)) return NOT_AVAILABLE;
  return `${formatAmount(value)}/unit-${grain}`;
}

/** `1,200.00/group-day` — a grouped design's own unit (the sample is the group). */
export function formatGroupDay(value: number | null, grain: Grain = "day"): string {
  if (!isNumber(value)) return NOT_AVAILABLE;
  return `${formatAmount(value)}/group-${grain}`;
}

/** As `formatUnitDay`, with an explicit `+` on a positive number. Zero is unsigned. */
export function formatSignedUnitDay(value: number | null, grain: Grain = "day"): string {
  if (!isNumber(value)) return NOT_AVAILABLE;
  return `${value > 0 ? "+" : ""}${formatUnitDay(value, grain)}`;
}

/** An amount in the target's unit, unsigned when positive — `3,057.60`, `-20.00`. */
export function formatAmount(value: number | null): string {
  if (!isNumber(value)) return NOT_AVAILABLE;
  return `${value < 0 ? "-" : ""}${TWO_DECIMALS.format(Math.abs(value))}`;
}

/** A total in the target's unit — `+3,057.60`, `-20.00`. */
export function formatSignedAmount(value: number | null): string {
  if (!isNumber(value)) return NOT_AVAILABLE;
  return `${value > 0 ? "+" : ""}${formatAmount(value)}`;
}

/** A share in [0, 1] as a one-decimal percent: `0.674` -> `67.4%`. */
export function formatShare(value: number | null): string {
  if (!isNumber(value)) return NOT_AVAILABLE;
  return `${(value * 100).toFixed(1)}%`;
}

/**
 * Treated compliance for display. With no treated entry read (`n_treated` 0) the
 * share is a 0 that measured nothing, so it is *not measured* rather than 0%.
 */
export function complianceLabel(c: { n_treated: number; c_treated: number }): {
  measured: boolean;
  text: string;
} {
  if (c.n_treated === 0) return { measured: false, text: "Compliance not measured" };
  return { measured: true, text: formatShare(c.c_treated) };
}

/** Control leakage for display: with no control entry read (`n_control` 0) the share measured nothing. */
export function leakLabel(c: { n_control: number; c_control_leak: number }): string {
  return c.n_control === 0 ? "not measured" : formatShare(c.c_control_leak);
}

/** A slope (target per driver unit): four significant figures, no currency. */
export function formatRatio(value: number | null): string {
  if (!isNumber(value)) return NOT_AVAILABLE;
  return String(Number(value.toPrecision(4)));
}

/**
 * An interval. `serde_json` writes an infinite endpoint as `null`, so a null (or
 * non-finite) endpoint means *unbounded* on that side — the engine could not
 * bound it. Rendering it as `0` would turn "we cannot say" into "the effect is zero".
 */
function bounds(low: number | null, high: number | null, format: (n: number) => string): string {
  const lo = isNumber(low) ? format(low) : "unbounded below";
  const hi = isNumber(high) ? format(high) : "unbounded above";
  return `${lo} to ${hi}`;
}

export const formatInterval = (low: number | null, high: number | null) =>
  bounds(low, high, formatSignedUnitDay);

export const formatAmountInterval = (low: number | null, high: number | null) =>
  bounds(low, high, formatSignedAmount);

export const formatRatioInterval = (low: number | null, high: number | null) =>
  bounds(low, high, formatRatio);

/** α as a bare number: `0.05`, `0.2`. Rounded to three significant figures to drop float noise. */
export function formatAlpha(alpha: number | null): string {
  if (!isNumber(alpha)) return NOT_AVAILABLE;
  return String(Number(alpha.toPrecision(3)));
}

export const formatSignedGroupDay = (value: number | null, grain: Grain = "day"): string => {
  if (!isNumber(value)) return NOT_AVAILABLE;
  return `${value > 0 ? "+" : ""}${formatGroupDay(value, grain)}`;
};

/** The figures of one estimate in one unit: plain, signed, and as an interval. */
export interface EffectFormat {
  plain: (value: number | null) => string;
  signed: (value: number | null) => string;
  interval: (low: number | null, high: number | null) => string;
}

/**
 * A ratio outcome's change per unit: a share, so percentage points, not money.
 * `+0.4 pp per unit`, matching `formatShare`'s one decimal. The sign follows the
 * rounded figure, so a change too small to show prints `0.0`, never `+0.0`.
 */
function formatShareChange(value: number | null, signed: boolean): string {
  if (!isNumber(value)) return NOT_AVAILABLE;
  const points = Math.round(value * 1000) / 10;
  const sign = points < 0 ? "-" : signed && points > 0 ? "+" : "";
  return `${sign}${Math.abs(points).toFixed(1)} pp per unit`;
}

/**
 * The unit a report's estimate is in. A design that assigns groups estimates
 * per group-day (the sample is the group); a unit design and a switchback (a fleet
 * mean) estimate per unit-day. Labelling a group estimate "per unit-day" would be
 * off by the mean group size, so the report picks its formatter from `design`.
 * `opts.share` (a ratio outcome) takes precedence over `design`: a share is per
 * unit whatever the design assigns.
 */
export function effectFormat(
  design: string,
  grain: Grain = "day",
  opts: { share?: boolean } = {}
): EffectFormat {
  if (opts.share) {
    const signed = (v: number | null) => formatShareChange(v, true);
    return {
      plain: (v) => formatShareChange(v, false),
      signed,
      interval: (low, high) => bounds(low, high, signed)
    };
  }
  if (design === "groups") {
    const signed = (v: number | null) => formatSignedGroupDay(v, grain);
    return {
      plain: (v) => formatGroupDay(v, grain),
      signed,
      interval: (low, high) => bounds(low, high, signed)
    };
  }
  const signed = (v: number | null) => formatSignedUnitDay(v, grain);
  return {
    plain: (v) => formatUnitDay(v, grain),
    signed,
    interval: (low, high) => bounds(low, high, signed)
  };
}
