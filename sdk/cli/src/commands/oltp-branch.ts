/**
 * An org's OLTP staging branch: `oxyc oltp reset --branch staging`, and the
 * staging line in `oltp status` / `oltp provision --branch staging`.
 *
 *   GET  /api/admin/orgs/{org_id}/oltp/branches/staging        status, age, apps a reset hits
 *   POST /api/admin/orgs/{org_id}/oltp/branches/staging/reset  {"confirm": true}
 *
 * Manual by ruling (env design §11 #16, 2026-09-29): one branch per ORG, made by
 * `oltp provision --branch staging`, reset only when someone asks — org-level,
 * confirmed, naming every app it affects — and never on a timer. Past 30 days
 * the server calls it stale; this prints the warning and leaves the decision to
 * the operator.
 */

import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { orgOrHint, resolveOrgId } from "./assume.js";
import { call, confirm, orgPath, PROVISION_TIMEOUT_MS } from "./oltp-client.js";

/** `oxy_oltp::branches::AffectedApp`. */
export interface AffectedApp {
  app_id: string;
  slug: string;
  name: string;
  schema: string;
}

/** `oxy_oltp::branches::AffectedPipeline`. */
export interface AffectedPipeline {
  source: string;
  schema: string;
}

/** `oxy_oltp::api::branches::BranchStatusResponse` — metadata, never a credential. */
export interface BranchStatus {
  branch: string;
  provisioned: boolean;
  status: string | null;
  host: string | null;
  database: string | null;
  provider_branch_id: string | null;
  created_at: string | null;
  last_reset_at: string | null;
  age_days: number | null;
  stale: boolean;
  stale_after_days: number;
  affected_apps: AffectedApp[];
  /** Airway pipelines' `raw_*` schemas, which a reset re-copies too. */
  affected_pipelines: AffectedPipeline[];
}

export type BranchName = "staging";

/** `--branch <name>` → the branch, refusing anything but the one there is. */
export function parseBranch(typed: string | undefined): BranchName | undefined {
  if (typed === undefined) return undefined;
  if (typed === "staging") return "staging";
  throw usageError(`--branch ${typed} is not an OLTP branch`, "the only one is `staging`");
}

export const branchPath = (orgId: string, branch: BranchName) =>
  `${orgPath(orgId)}/branches/${branch}`;

export function fetchBranch(
  ctx: Context,
  orgId: string,
  branch: BranchName
): Promise<BranchStatus> {
  return call<BranchStatus>(ctx, "GET", branchPath(orgId, branch));
}

/** The staging line(s) of `oltp status`: where, how old, and whether to worry. */
export function branchLines(org: string, b: BranchStatus): string[] {
  if (!b.provisioned) {
    return [`${b.branch.padEnd(9)} none — oxyc oltp provision --org ${org} --branch ${b.branch}`];
  }
  const since = b.last_reset_at ? "reset" : "cut";
  const age = b.age_days === null ? "" : `, ${since} ${b.age_days} day(s) ago`;
  const lines = [`${b.branch.padEnd(9)} ${b.database} on ${b.host} — ${b.status}${age}`];
  if (b.stale) {
    lines.push(
      out.yellow(
        `          STALE: older than ${b.stale_after_days} days — rows deleted in production since ` +
          `may live on here. Reset: oxyc oltp reset --org ${org} --branch ${b.branch}`
      )
    );
  }
  return lines;
}

function reachLines(apps: AffectedApp[], pipelines: AffectedPipeline[]): string[] {
  const lines = apps.map((a) => `  app       ${a.slug} (${a.name}) — ${a.schema}`);
  for (const p of pipelines) lines.push(`  pipeline  ${p.source} (Airway) — ${p.schema}`);
  if (lines.length === 0)
    return ["  apps      none has an OLTP writer — nothing of theirs is lost"];
  return lines;
}

/** On stderr, like `provision`'s plan: this is what the prompt asks about. */
function printResetPlan(target: string, org: string, orgId: string, b: BranchStatus): void {
  const lines = [
    `On ${target}, for org ${org === orgId ? orgId : `${org} (${orgId})`}:`,
    `  branch    ${b.branch}: ${b.database} on ${b.host} — re-copied from production`,
    "  discards  every staging write in the org's OLTP branch, for EVERY app and pipeline below:",
    ...reachLines(b.affected_apps, b.affected_pipelines ?? [])
  ];
  process.stderr.write(`${lines.join("\n")}\n`);
}

export async function runOltpReset(
  ctx: Context,
  org: string | undefined,
  flags: { branch?: string; yes?: boolean; json?: boolean }
): Promise<void> {
  // Usage first — no request for a command that cannot run. Production is
  // never a branch, so there is no default to fall back on.
  const branch = parseBranch(flags.branch);
  if (branch === undefined) {
    throw usageError("name the branch to reset: --branch staging", "production is never reset");
  }
  const orgId = await resolveOrgId(ctx, org);
  const named = orgOrHint(ctx, org);
  const current = await fetchBranch(ctx, orgId, branch);
  if (!current.provisioned) {
    throw new CliError(`org ${named} has no ${branch} branch — nothing to reset`, {
      code: ExitCode.NOT_FOUND,
      hint: `oxyc oltp provision --org ${named} --branch ${branch}`
    });
  }
  printResetPlan(ctx.target(), named, orgId, current);
  if (!flags.yes) {
    await confirm("Reset?", {
      verb: "a branch reset",
      why: "it discards every app's staging data in the org at once",
      declined: "not reset — the confirmation was declined"
    });
  }

  const result = await call<BranchStatus & { reset: boolean }>(
    ctx,
    "POST",
    `${branchPath(orgId, branch)}/reset`,
    { body: { confirm: true }, timeoutMs: PROVISION_TIMEOUT_MS }
  );
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
    return;
  }
  process.stdout.write(
    `${out.green(`reset ${branch}: ${result.database} re-copied from production`)}\n`
  );
  process.stdout.write(`${branchLines(named, result).join("\n")}\n`);
  log.info(`${result.affected_apps.length} app(s) start staging from production's data again`);
}
