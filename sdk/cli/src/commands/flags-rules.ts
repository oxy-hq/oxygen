/**
 * The rules half of `oxyc flags`, pure: no I/O, so every rule of the grammar is
 * testable without a server.
 *
 * The wire shapes mirror `crates/app/src/server/feature_flags/targeting.rs`
 * (and `web-app/src/services/api/featureFlags.ts`). The server stays the
 * authority on what is valid; what is checked here is what can be said better
 * before a round trip — a typo'd variant, a split that does not add up — with a
 * usage exit code rather than a 422.
 */

import { usageError } from "../util/errors.js";

/** Split weights are basis points and must sum to exactly this (100.00%). */
export const TOTAL_WEIGHT = 10_000;

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export type Fallthrough =
  | { type: "variant"; variant: string }
  | { type: "split"; weights: Record<string, number> };

export interface Targeting {
  users: Record<string, string>;
  orgs: Record<string, string>;
  fallthrough: Fallthrough;
  salt?: string;
}

export interface FeatureFlag {
  key: string;
  description: string;
  default: boolean;
  enabled: boolean;
  updated_at: string | null;
  variants: string[];
  unit: "org" | "user";
  per_org: boolean;
  client: boolean;
  targeting: Targeting | null;
  targeting_error: string | null;
}

/** One `--set-org` / `--set-user` value: `<id>=<variant>`; an empty variant removes the override. */
export interface Assignment {
  id: string;
  /** `null`: remove this override. */
  variant: string | null;
}

/** `<id>=<variant>`, split at the first `=`. `<id>=` (nothing after) removes it. */
export function parseAssignment(raw: string, flag: string): Assignment {
  const at = raw.indexOf("=");
  if (at < 1) {
    throw usageError(`${flag} takes <id>=<variant>, got '${raw}'`, `${flag} acme=treatment`);
  }
  const id = raw.slice(0, at).trim();
  const variant = raw.slice(at + 1).trim();
  return { id, variant: variant === "" ? null : variant };
}

/**
 * `control=50,treatment=50` — percentages, as a person says them — into basis
 * points. Whole and fractional percents both work (`33.33`), but a share that
 * does not land on a basis point (`33.333`) is refused rather than rounded, and
 * so is anything that does not total 100: the server wants exactly 10000, and a
 * silent fix-up would send a split nobody typed.
 */
export function parseSplit(raw: string): Record<string, number> {
  const weights: Record<string, number> = {};
  for (const part of raw.split(",")) {
    const piece = part.trim();
    if (!piece) continue;
    const at = piece.indexOf("=");
    const variant = at < 1 ? "" : piece.slice(0, at).trim();
    const pct = at < 1 ? "" : piece.slice(at + 1).trim();
    if (!variant || !/^\d+(\.\d+)?$/.test(pct)) {
      throw usageError(
        `--split takes <variant>=<percent>,… — got '${piece}'`,
        "--split control=50,treatment=50"
      );
    }
    if (variant in weights) throw usageError(`--split names '${variant}' twice`);
    const points = Number(pct) * 100;
    const whole = Math.round(points);
    if (Math.abs(points - whole) > 1e-6) {
      throw usageError(
        `${variant}=${pct}% is finer than 0.01%`,
        "weights are basis points: use at most two decimals"
      );
    }
    weights[variant] = whole;
  }
  if (Object.keys(weights).length === 0) {
    throw usageError("--split needs at least one variant", "--split control=50,treatment=50");
  }
  const total = Object.values(weights).reduce((a, b) => a + b, 0);
  if (total !== TOTAL_WEIGHT) {
    throw usageError(
      `--split adds up to ${total / 100}%, not 100%`,
      "the shares must total exactly 100, e.g. control=50,treatment=50"
    );
  }
  return weights;
}

/** What the caller asked to change. Every field is optional; none means no edit. */
export interface RuleEdits {
  /** Org overrides by ORG UUID (the caller has resolved slugs); `null` removes. */
  orgs: Record<string, string | null>;
  /** User overrides by user UUID; `null` removes. */
  users: Record<string, string | null>;
  /** Everyone not overridden gets this one variant. */
  everyone?: string;
  /** Everyone not overridden is split by these basis-point weights. */
  split?: Record<string, number>;
  /** Clear every rule: the flag goes back to the plain kill switch. */
  clear?: boolean;
  /** Replace the rules wholesale. */
  replace?: Targeting;
}

export function hasEdits(edits: RuleEdits): boolean {
  return (
    Object.keys(edits.orgs).length > 0 ||
    Object.keys(edits.users).length > 0 ||
    edits.everyone !== undefined ||
    edits.split !== undefined ||
    edits.clear === true ||
    edits.replace !== undefined
  );
}

/**
 * What a flag with no rules serves everyone — the fallthrough that preserves
 * today's behaviour when the first override is added to it. The kill switch on
 * is `variants[1]` ("on", or the treatment of a multi-variant flag); off is
 * `variants[0]`.
 */
export function implicitFallthrough(flag: FeatureFlag): Fallthrough {
  return {
    type: "variant",
    variant: (flag.enabled ? flag.variants[1] : undefined) ?? flag.variants[0] ?? "off"
  };
}

function assertVariant(flag: FeatureFlag, variant: string, where: string): void {
  if (!flag.variants.includes(variant)) {
    throw usageError(
      `'${variant}' is not a variant of ${flag.key} (${where}); it has ${flag.variants.join(", ")}`
    );
  }
}

function assertUuid(id: string, what: string): void {
  if (!UUID.test(id)) throw usageError(`${what} '${id}' is not a UUID`);
}

function applyOverrides(
  current: Record<string, string>,
  edits: Record<string, string | null>,
  flag: FeatureFlag,
  what: string
): Record<string, string> {
  const next = { ...current };
  for (const [id, variant] of Object.entries(edits)) {
    if (variant === null) {
      delete next[id];
      continue;
    }
    assertVariant(flag, variant, `${what} ${id}`);
    next[id] = variant;
  }
  return next;
}

/**
 * Read-modify-write: the rules to PATCH, from the flag as the server has it and
 * the edits asked for. An override added here leaves every other override, the
 * fallthrough and the salt as they were — which is the point, since retyping a
 * whole rule set to add one org is how a rule gets lost.
 *
 * `null` means "clear the rules". Throws a usage error for an edit that cannot
 * be made (`--clear` with other edits, an unknown variant, a split naming a
 * variant the flag does not have).
 */
export function applyRuleEdits(flag: FeatureFlag, edits: RuleEdits): Targeting | null {
  if (edits.everyone !== undefined && edits.split !== undefined) {
    throw usageError("--everyone and --split both set the fallthrough; pass one");
  }
  const others =
    Object.keys(edits.orgs).length > 0 ||
    Object.keys(edits.users).length > 0 ||
    edits.everyone !== undefined ||
    edits.split !== undefined;
  if (edits.replace && (others || edits.clear)) {
    throw usageError("--file replaces the rules wholesale; pass it alone");
  }
  if (edits.clear && others) {
    throw usageError("--clear removes every rule; it cannot be combined with other edits");
  }
  if (edits.clear) return null;
  if (edits.replace) return checked(flag, edits.replace);

  const base: Targeting = flag.targeting ?? {
    users: {},
    orgs: {},
    fallthrough: implicitFallthrough(flag)
  };
  for (const id of Object.keys(edits.users)) assertUuid(id, "user");
  for (const id of Object.keys(edits.orgs)) assertUuid(id, "org");

  const next: Targeting = {
    ...base,
    users: applyOverrides(base.users, edits.users, flag, "user"),
    orgs: applyOverrides(base.orgs, edits.orgs, flag, "org")
  };
  if (edits.everyone !== undefined) next.fallthrough = { type: "variant", variant: edits.everyone };
  if (edits.split !== undefined) next.fallthrough = { type: "split", weights: edits.split };
  return checked(flag, next);
}

/** The flag-aware checks the server will make again: variants known, split totals 100%. */
function checked(flag: FeatureFlag, rules: Targeting): Targeting {
  if (!rules || typeof rules !== "object" || !rules.fallthrough) {
    throw usageError(
      "the rules need a fallthrough",
      'e.g. {"users":{},"orgs":{},"fallthrough":{"type":"variant","variant":"off"}}'
    );
  }
  rules = { ...rules, users: rules.users ?? {}, orgs: rules.orgs ?? {} };
  for (const [id, variant] of Object.entries(rules.users)) {
    assertUuid(id, "user");
    assertVariant(flag, variant, `user ${id}`);
  }
  for (const [id, variant] of Object.entries(rules.orgs)) {
    assertUuid(id, "org");
    assertVariant(flag, variant, `org ${id}`);
  }
  const fall = rules.fallthrough;
  if (fall.type === "variant") {
    assertVariant(flag, fall.variant, "fallthrough");
  } else if (fall.type === "split") {
    for (const variant of Object.keys(fall.weights)) assertVariant(flag, variant, "split");
    const total = Object.values(fall.weights).reduce((a, b) => a + b, 0);
    if (total !== TOTAL_WEIGHT) {
      throw usageError(`split weights sum to ${total}; they must sum to ${TOTAL_WEIGHT} (100.00%)`);
    }
  } else {
    throw usageError("the fallthrough's type must be 'variant' or 'split'");
  }
  return rules;
}
