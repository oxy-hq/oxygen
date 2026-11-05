/**
 * `oxyc flags` — feature flags and A/B splits from a terminal.
 *
 * Flags are declared in code; what an admin changes at runtime is the kill
 * switch and the targeting rules (per-org / per-user overrides and a weighted
 * fallthrough). Each verb is a thin client over one route:
 *
 *   list                         GET   /api/admin/feature-flags
 *   show <key>                   GET   /api/admin/feature-flags            (one row)
 *   set <key> --on|--off         PATCH /api/admin/feature-flags/{key}      {enabled}
 *   rules <key> …                PATCH /api/admin/feature-flags/{key}      {targeting}
 *   eval [--org] [--user]        GET   /api/admin/feature-flags/evaluate
 *   org <org>                    GET   /api/orgs/{org_id}/feature-flags
 *
 * Every write is deployment-wide and takes effect for real orgs, so each one
 * reads the flag first, says what will change, and asks (or wants `--yes`).
 */

import type { Context } from "../context/resolve.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { resolveOrgId } from "./assume.js";
import { call, evaluatePath, flagsPath, orgFlagsPath, readRulesFile } from "./flags-client.js";
import {
  type Evaluation,
  type OrgFlags,
  printEvaluations,
  printFlag,
  printFlagList,
  printOrgFlags,
  printRules,
  rulesSummary
} from "./flags-render.js";
import {
  type Assignment,
  applyRuleEdits,
  type FeatureFlag,
  hasEdits,
  parseAssignment,
  parseSplit,
  type RuleEdits,
  type Targeting
} from "./flags-rules.js";
import { confirm } from "./oltp-client.js";

function emit(value: unknown): void {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}
function say(line: string): void {
  process.stdout.write(`${line}\n`);
}

async function readFlags(ctx: Context): Promise<FeatureFlag[]> {
  return call<FeatureFlag[]>(ctx, "GET", flagsPath());
}

async function readFlag(ctx: Context, key: string): Promise<FeatureFlag> {
  const flags = await readFlags(ctx);
  const found = flags.find((f) => f.key === key);
  if (found) return found;
  throw new CliError(`no feature flag named '${key}'`, {
    code: ExitCode.NOT_FOUND,
    hint: `declared flags: ${flags.map((f) => f.key).join(", ") || "none"} — flags are declared in code (registry.rs), not created at runtime`
  });
}

export async function runFlagsList(ctx: Context, opts: { json: boolean }): Promise<void> {
  const flags = await readFlags(ctx);
  if (opts.json) {
    emit(flags);
    return;
  }
  printFlagList(flags);
}

export async function runFlagsShow(
  ctx: Context,
  key: string,
  opts: { json: boolean }
): Promise<void> {
  const flag = await readFlag(ctx, key);
  if (opts.json) {
    emit(flag);
    return;
  }
  printFlag(flag);
}

/** The line after a successful write: what the flag now does, and a warning if the rules sit behind an off switch. */
function reportUpdated(flag: FeatureFlag, json: boolean): void {
  if (json) {
    emit(flag);
    return;
  }
  say(`updated ${flag.key}: kill switch ${flag.enabled ? "on" : "off"}; ${rulesSummary(flag)}`);
  if (!flag.enabled && flag.targeting) {
    say(
      `note: the kill switch is off, so every org gets ${flag.variants[0]} and the rules do not apply until \`oxyc flags set ${flag.key} --on\``
    );
  }
}

export async function runFlagsSet(
  ctx: Context,
  key: string,
  opts: { on?: boolean; off?: boolean; yes: boolean; json: boolean }
): Promise<void> {
  if (opts.on === opts.off || (!opts.on && !opts.off)) {
    throw usageError(`pass exactly one of --on or --off`, `oxyc flags set ${key} --off`);
  }
  const enabled = Boolean(opts.on);
  const flag = await readFlag(ctx, key);
  if (flag.enabled === enabled) {
    if (!opts.json) say(`${key} kill switch is already ${enabled ? "on" : "off"} — nothing sent`);
    else emit(flag);
    return;
  }
  if (!opts.yes) {
    await confirm(
      `Turn the ${key} kill switch ${enabled ? "on" : "off"} for the whole deployment?`,
      {
        verb: "set",
        why: enabled
          ? "on: orgs get the flag per its rules"
          : `off: every org gets ${flag.variants[0]}, whatever the rules say`,
        declined: "set declined — nothing was sent"
      }
    );
  }
  reportUpdated(await call<FeatureFlag>(ctx, "PATCH", flagsPath(key), { enabled }), opts.json);
}

export interface RulesOptions {
  setOrg: string[];
  setUser: string[];
  removeOrg: string[];
  removeUser: string[];
  everyone?: string;
  split?: string;
  file?: string;
  clear: boolean;
  dryRun: boolean;
  yes: boolean;
  json: boolean;
}

async function orgOverrides(
  ctx: Context,
  sets: Assignment[],
  removals: string[]
): Promise<Record<string, string | null>> {
  const out: Record<string, string | null> = {};
  for (const a of sets) out[await resolveOrgId(ctx, a.id)] = a.variant;
  for (const org of removals) out[await resolveOrgId(ctx, org)] = null;
  return out;
}

function userOverrides(sets: Assignment[], removals: string[]): Record<string, string | null> {
  const out: Record<string, string | null> = {};
  for (const a of sets) out[a.id] = a.variant;
  for (const id of removals) out[id.trim()] = null;
  return out;
}

async function collectEdits(ctx: Context, opts: RulesOptions): Promise<RuleEdits> {
  const edits: RuleEdits = {
    orgs: await orgOverrides(
      ctx,
      opts.setOrg.map((v) => parseAssignment(v, "--set-org")),
      opts.removeOrg
    ),
    users: userOverrides(
      opts.setUser.map((v) => parseAssignment(v, "--set-user")),
      opts.removeUser
    ),
    everyone: opts.everyone,
    split: opts.split === undefined ? undefined : parseSplit(opts.split),
    clear: opts.clear || undefined,
    replace: opts.file === undefined ? undefined : readRulesFile(opts.file)
  };
  return edits;
}

function describeChange(flag: FeatureFlag, next: Targeting | null): string {
  const after: FeatureFlag = { ...flag, targeting: next, targeting_error: null };
  return `${flag.key}\n  now:   ${rulesSummary(flag)}\n  after: ${rulesSummary(after)}`;
}

export async function runFlagsRules(ctx: Context, key: string, opts: RulesOptions): Promise<void> {
  const edits = await collectEdits(ctx, opts);
  if (!hasEdits(edits)) {
    throw usageError(
      "nothing to change",
      "--set-org <org>=<variant>, --set-user <uuid>=<variant>, --everyone <variant>, --split a=50,b=50, --file rules.json or --clear"
    );
  }
  const flag = await readFlag(ctx, key);
  if (flag.targeting_error && !edits.clear && !edits.replace) {
    throw new CliError(`${key} has stored rules this build cannot read`, {
      code: ExitCode.REFUSED,
      detail: flag.targeting_error,
      remedy: `replace them with --file rules.json, or discard them with --clear`
    });
  }
  const next = applyRuleEdits(flag, edits);
  if (opts.dryRun) {
    if (opts.json) {
      emit({ key, targeting: next });
      return;
    }
    say(`${key}: these rules would be sent (dry run, nothing was sent)`);
    if (next) printRules(next, flag.variants);
    else say("no rules (cleared)");
    return;
  }
  if (!opts.yes) {
    await confirm(
      `Change ${key}'s rules on the whole deployment?\n${describeChange(flag, next)}\n`,
      {
        verb: "rules",
        why: "rules take effect for real orgs the moment they are saved",
        declined: "rules declined — nothing was sent"
      }
    );
  }
  reportUpdated(
    await call<FeatureFlag>(ctx, "PATCH", flagsPath(key), { targeting: next }),
    opts.json
  );
}

export async function runFlagsEval(
  ctx: Context,
  opts: { org?: string; user?: string; json: boolean }
): Promise<void> {
  const orgId = opts.org === undefined ? undefined : await resolveOrgId(ctx, opts.org);
  const rows = await call<Evaluation[]>(ctx, "GET", evaluatePath(orgId, opts.user?.trim()));
  if (opts.json) {
    emit(rows);
    return;
  }
  printEvaluations(rows);
}

export async function runFlagsOrg(
  ctx: Context,
  org: string | undefined,
  opts: { json: boolean }
): Promise<void> {
  const body = await call<OrgFlags>(ctx, "GET", orgFlagsPath(await resolveOrgId(ctx, org)));
  if (opts.json) {
    emit(body);
    return;
  }
  printOrgFlags(body);
}
