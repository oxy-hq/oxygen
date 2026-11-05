/**
 * How flags read in a terminal. `--json` is the server's own body; this is the
 * human/agent summary. A uuid stays a uuid here: rules store org ids, and
 * resolving each back to a slug would be a lookup per row for a view that is
 * mostly pasted back into another command.
 */

import { table } from "../ui/render.js";
import type { Fallthrough, FeatureFlag, Targeting } from "./flags-rules.js";

function say(line: string): void {
  process.stdout.write(`${line}\n`);
}

export interface Evaluation {
  key: string;
  variant: string;
  reason: string;
  bucket?: number | null;
}

export interface OrgFlags {
  flags: Record<string, { variant: string; enabled: boolean }>;
}

const plural = (n: number, noun: string) => `${n} ${noun}${n === 1 ? "" : "s"}`;
const percent = (bp: number) => `${bp / 100}%`;

/** `everyone: on`, or `split control 50% / treatment 50%` in the flag's own variant order. */
export function describeFallthrough(fallthrough: Fallthrough, variants: string[] = []): string {
  if (fallthrough.type === "variant") return `everyone else: ${fallthrough.variant}`;
  const names = [
    ...variants.filter((v) => v in fallthrough.weights),
    ...Object.keys(fallthrough.weights).filter((v) => !variants.includes(v))
  ];
  return `split ${names.map((v) => `${v} ${percent(fallthrough.weights[v] ?? 0)}`).join(" / ")}`;
}

/** One cell: `2 orgs, 1 user, split control 50% / treatment 50%`. */
export function rulesSummary(flag: FeatureFlag): string {
  if (flag.targeting_error) return "UNREADABLE RULES";
  const t = flag.targeting;
  if (!t) return flag.per_org ? "no rules" : "global only";
  const parts: string[] = [];
  const orgs = Object.keys(t.orgs).length;
  const users = Object.keys(t.users).length;
  if (orgs) parts.push(plural(orgs, "org"));
  if (users) parts.push(plural(users, "user"));
  parts.push(describeFallthrough(t.fallthrough, flag.variants));
  return parts.join(", ");
}

export function printFlagList(flags: FeatureFlag[]): void {
  if (flags.length === 0) {
    process.stderr.write("no feature flags declared on this deployment\n");
    return;
  }
  process.stdout.write(
    `${table(flags, [
      { header: "key", value: (f) => f.key },
      { header: "kill switch", value: (f) => (f.enabled ? "on" : "off") },
      { header: "rules", value: rulesSummary },
      { header: "unit", value: (f) => f.unit },
      {
        header: "scope",
        value: (f) =>
          [f.per_org ? "per-org" : "global", f.client ? "client" : ""].filter(Boolean).join(", ")
      }
    ])}\n`
  );
}

function printOverrides(label: string, overrides: Record<string, string>): void {
  const entries = Object.entries(overrides);
  if (entries.length === 0) return;
  say(`${label}`);
  for (const [id, variant] of entries) say(`  ${id}  ${variant}`);
}

/** The rules as a person would read them, for `show` and the `rules` preview. */
export function printRules(t: Targeting, variants: string[]): void {
  printOverrides("orgs", t.orgs);
  printOverrides("users", t.users);
  say(describeFallthrough(t.fallthrough, variants));
  if (t.salt) say(`salt: ${t.salt}`);
}

export function printFlag(flag: FeatureFlag): void {
  say(flag.key);
  if (flag.description) say(flag.description);
  say("");
  const kill = flag.enabled
    ? `on (serves ${flag.variants.slice(1).join(" / ") || flag.variants[0]} per the rules)`
    : `off (every org gets ${flag.variants[0]})`;
  say(`kill switch   ${kill}`);
  say(`variants      ${flag.variants.join(", ")}  (${flag.variants[0]} is off/control)`);
  say(`unit          ${flag.unit}`);
  say(
    `scope         ${flag.per_org ? "per-org (rules allowed)" : "global only (rules refused)"}${flag.client ? ", client-readable" : ""}`
  );
  say(`default       ${flag.default ? "on" : "off"}`);
  if (flag.updated_at) say(`updated       ${flag.updated_at}`);
  say("");
  if (flag.targeting_error) {
    say(`UNREADABLE RULES: ${flag.targeting_error}`);
    say(
      "The server serves the off variant until the rules are replaced: `oxyc flags rules <key> --file rules.json` or --clear."
    );
    return;
  }
  if (!flag.targeting) {
    say("rules         none");
    return;
  }
  say("rules");
  printRules(flag.targeting, flag.variants);
}

export function printEvaluations(rows: Evaluation[]): void {
  if (rows.length === 0) {
    process.stderr.write("no feature flags declared on this deployment\n");
    return;
  }
  process.stdout.write(
    `${table(rows, [
      { header: "key", value: (r) => r.key },
      { header: "variant", value: (r) => r.variant },
      { header: "reason", value: (r) => r.reason },
      { header: "bucket", value: (r) => (r.bucket == null ? "" : String(r.bucket)), align: "right" }
    ])}\n`
  );
}

export function printOrgFlags(body: OrgFlags): void {
  const rows = Object.entries(body.flags ?? {})
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([key, v]) => ({ key, ...v }));
  if (rows.length === 0) {
    process.stderr.write("no client-readable flags for this org\n");
    return;
  }
  process.stdout.write(
    `${table(rows, [
      { header: "key", value: (r) => r.key },
      { header: "variant", value: (r) => r.variant },
      { header: "enabled", value: (r) => (r.enabled ? "yes" : "no") }
    ])}\n`
  );
}
