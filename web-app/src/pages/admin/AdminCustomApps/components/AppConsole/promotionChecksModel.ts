import type { PromotionChecks, PromotionChecksUpdate } from "@/types/apps";

/**
 * Which checks hold an app's Promote, read off the app (`promotion_checks`) and
 * written back (`PUT …/promotion-checks`). Kept as data so the settings rows, their
 * defaults and the request each switch sends are tested rather than typed into JSX.
 *
 * The gate is the org's `promote_checks_gate` flag: `promotionGate` answers `null`
 * while it is off, and every surface that asks shows what it showed before.
 */

/** The checks an app can require, in the order the settings list them. */
export const REQUIRABLE_CHECKS = [
  { kind: "functions", label: "Its checks pass" },
  { kind: "loads", label: "It loads, desk and phone" },
  { kind: "design_system", label: "Follows the design system" },
  { kind: "smoke", label: "Every screen opens" }
] as const;

/** What `required: null` comes to when the server does not say (`effective_required`). */
const DEFAULT_REQUIRED: string[] = REQUIRABLE_CHECKS.map((c) => c.kind);

/** The three switches that are not a check. */
export const PROMOTION_SWITCHES = [
  { field: "seen_required", label: "A person has seen what changed" },
  { field: "production_smoke", label: "Smoke production after promote" },
  { field: "auto_rollback", label: "Roll back automatically if it fails" }
] as const;
export type PromotionField = (typeof PROMOTION_SWITCHES)[number]["field"];

export interface PromotionGate {
  /** What the app stored: its own list, or `null` for the defaults. */
  required: string[] | null;
  /** The kinds that hold Promote now. */
  effective: string[];
  seen_required: boolean;
  production_smoke: boolean;
  auto_rollback: boolean;
}

/** The app's gate, or `null` while the org's flag is off (or the server predates it).
 *  Every field is read with its documented default, so a partial answer still reads. */
export function promotionGate(checks: PromotionChecks | null | undefined): PromotionGate | null {
  if (checks?.enabled !== true) return null;
  const required = Array.isArray(checks.required) ? checks.required : null;
  return {
    required,
    effective: Array.isArray(checks.effective_required)
      ? checks.effective_required
      : (required ?? DEFAULT_REQUIRED),
    seen_required: checks.seen_required === true,
    production_smoke: checks.production_smoke !== false,
    auto_rollback: checks.auto_rollback === true
  };
}

const stored = (gate: PromotionGate): PromotionChecksUpdate => ({
  required: gate.required,
  seen_required: gate.seen_required,
  production_smoke: gate.production_smoke,
  auto_rollback: gate.auto_rollback
});

/**
 * The request that turns one check's requirement on or off. The list becomes the
 * app's own (no longer `null`): the kinds that hold now, with this one added or
 * taken out, in the settings' order and then any kind this console does not list.
 */
export function requireCheck(
  gate: PromotionGate,
  kind: string,
  on: boolean
): PromotionChecksUpdate {
  const wanted = new Set(gate.effective);
  if (on) wanted.add(kind);
  else wanted.delete(kind);
  const known = DEFAULT_REQUIRED.filter((k) => wanted.has(k));
  const others = [...wanted].filter((k) => !DEFAULT_REQUIRED.includes(k));
  return { ...stored(gate), required: [...known, ...others] };
}

/** The request that flips one of the other switches. `required` goes back as it was
 *  stored, so an app on the defaults stays on them. */
export function setPromotionField(
  gate: PromotionGate,
  field: PromotionField,
  on: boolean
): PromotionChecksUpdate {
  return { ...stored(gate), [field]: on };
}

/** What the settings show while a save is in flight: where it is going. */
export function gateAfter(gate: PromotionGate, update: PromotionChecksUpdate): PromotionGate {
  return {
    required: update.required,
    effective: update.required ?? gate.effective,
    seen_required: update.seen_required,
    production_smoke: update.production_smoke,
    auto_rollback: update.auto_rollback
  };
}
