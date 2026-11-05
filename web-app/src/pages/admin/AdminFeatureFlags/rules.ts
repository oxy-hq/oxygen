import type { FeatureFlag, Targeting } from "@/services/api/featureFlags";

/** A split totals 100% = 10000 basis points. */
export const TOTAL_BPS = 10_000;

const PERCENT_RE = /^\d{1,3}(\.\d{1,2})?$/;
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export const isUuid = (value: string): boolean => UUID_RE.test(value.trim());

/**
 * "33.33" -> 3333. Blank counts as 0; anything else that is not 0–100 with at most
 * two decimals is `null`. Parsed from the text, not multiplied, so no float drift.
 */
export function percentToBps(text: string): number | null {
  const t = text.trim();
  if (t === "") return 0;
  if (!PERCENT_RE.test(t)) return null;
  const [whole, frac = ""] = t.split(".");
  const bps = Number(whole) * 100 + Number(frac.padEnd(2, "0"));
  return bps <= TOTAL_BPS ? bps : null;
}

/** 3333 -> "33.33", 5000 -> "50". */
export function bpsToPercent(bps: number): string {
  return String(Number((bps / 100).toFixed(2)));
}

/** Sum of the inputs in basis points, or `null` if any input is invalid. */
export function splitTotalBps(percents: Record<string, string>): number | null {
  let total = 0;
  for (const text of Object.values(percents)) {
    const bps = percentToBps(text);
    if (bps === null) return null;
    total += bps;
  }
  return total;
}

export const isSplitValid = (percents: Record<string, string>): boolean =>
  splitTotalBps(percents) === TOTAL_BPS;

/** The "Everyone else" half of the editor, plus the override lists. */
export interface RulesDraft {
  mode: "variant" | "split";
  variant: string;
  /** Percent text per variant, as typed. */
  percents: Record<string, string>;
  orgs: Record<string, string>;
  users: Record<string, string>;
}

export function draftFromFlag(flag: FeatureFlag): RulesDraft {
  const t = flag.targeting;
  const fallthrough = t?.fallthrough;
  const percents: Record<string, string> = {};
  for (const v of flag.variants) percents[v] = "0";
  let variant = flag.default ? (flag.variants[1] ?? flag.variants[0]) : flag.variants[0];
  if (fallthrough?.type === "variant") variant = fallthrough.variant;
  if (fallthrough?.type === "split") {
    for (const [v, bps] of Object.entries(fallthrough.weights)) percents[v] = bpsToPercent(bps);
  }
  return {
    mode: fallthrough?.type === "split" ? "split" : "variant",
    variant,
    percents,
    orgs: { ...t?.orgs },
    users: { ...t?.users }
  };
}

/** The `Targeting` to send. Call only when the draft is valid (`isSplitValid` in split mode). */
export function buildTargeting(flag: FeatureFlag, draft: RulesDraft): Targeting {
  const weights: Record<string, number> = {};
  for (const v of flag.variants) weights[v] = percentToBps(draft.percents[v] ?? "") ?? 0;
  return {
    orgs: draft.orgs,
    users: draft.users,
    fallthrough:
      draft.mode === "split"
        ? { type: "split", weights }
        : { type: "variant", variant: draft.variant },
    ...(flag.targeting?.salt ? { salt: flag.targeting.salt } : {})
  };
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

function fallthroughSummary(flag: FeatureFlag, t: Targeting, hasOverrides: boolean): string {
  const f = t.fallthrough;
  if (f.type === "variant") return `${hasOverrides ? "Rest" : "Everyone"}: ${f.variant}`;
  const names = [
    ...flag.variants,
    ...Object.keys(f.weights).filter((v) => !flag.variants.includes(v))
  ];
  // An on/off-style flag reads best as the share that gets the non-control variant.
  if (names.length === 2) {
    const share = f.weights[names[1]] ?? 0;
    if (names[0] === "off" || names[0] === "control") {
      return `${bpsToPercent(share)}% ${names[1]}`;
    }
  }
  const nums = names.map((v) => bpsToPercent(f.weights[v] ?? 0)).join("/");
  return `${nums} ${names.join("·")}`;
}

/** One short line for the Rules column. */
export function summarizeRules(flag: FeatureFlag): string {
  const t = flag.targeting;
  if (!flag.per_org || !t) return "Global";
  const orgs = Object.keys(t.orgs).length;
  const users = Object.keys(t.users).length;
  const parts: string[] = [];
  if (orgs > 0) parts.push(plural(orgs, "org"));
  if (users > 0) parts.push(plural(users, "user"));
  parts.push(fallthroughSummary(flag, t, parts.length > 0));
  return parts.join(" · ");
}

/** What a flag serves right now, for the list: a lead line and an optional second. */
export interface Serving {
  lead: string;
  detail?: string;
  /** The stored rules can't be served, so everyone gets the off variant. */
  broken?: boolean;
}

export function servingSummary(flag: FeatureFlag): Serving {
  const off = flag.variants[0] ?? "off";
  if (flag.targeting_error) {
    return { lead: `Everyone: ${off}`, detail: "Stored rules unreadable", broken: true };
  }
  const rules = flag.per_org && flag.targeting ? summarizeRules(flag) : undefined;
  if (!flag.enabled) {
    return { lead: "Off for everyone", detail: rules && `Rules kept: ${rules}` };
  }
  if (!rules) return { lead: "On for everyone" };
  return { lead: rules };
}

/** The server's evaluation reason, in words. */
export function reasonLabel(reason: string, bucket?: number): string {
  switch (reason) {
    case "STATIC":
      return "Global switch";
    case "DISABLED":
      return "Switched off";
    case "TARGETING_MATCH":
      return "Override";
    case "SPLIT":
      return bucket === undefined ? "Split" : `Split, bucket ${bucket}`;
    case "DEFAULT":
      return "Everyone else";
    case "ERROR":
      return "Error: served off";
    default:
      return reason;
  }
}

export interface OrgOption {
  id: string;
  name: string;
}

/** An org id from what was typed: a pasted uuid, or an exact (case-blind) org name. */
export function resolveOrgId(text: string, orgs: OrgOption[]): string | null {
  const t = text.trim();
  if (isUuid(t)) return t.toLowerCase();
  const byName = orgs.filter((o) => o.name.toLowerCase() === t.toLowerCase());
  return byName.length === 1 ? byName[0].id : null;
}

/**
 * Why a typed org name did not resolve. The org list is drained page by page and can
 * fail part way, so a name missing from it is only absent once every page has arrived;
 * before that, or after a failed page, the honest answer is "can't tell yet".
 */
export type OrgMiss = "ambiguous" | "unavailable" | "loading" | "absent";

export function orgMiss(
  text: string,
  orgs: OrgOption[],
  list: { isLoading: boolean; isIncomplete: boolean }
): OrgMiss {
  const t = text.trim().toLowerCase();
  if (orgs.filter((o) => o.name.toLowerCase() === t).length > 1) return "ambiguous";
  if (list.isIncomplete) return "unavailable";
  if (list.isLoading) return "loading";
  return "absent";
}
