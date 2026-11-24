/**
 * `oxyc oltp apply` — apply a workspace's compiled `schemas/*.sql` that the
 * org's OLTP database has not run, and re-grant every writer over its schema.
 *
 *   POST /api/admin/orgs/{org_id}/oltp/apply    {"workspace_id"?: uuid}
 *
 * It replaced the Rust `oxy oltp apply`, which read `OXY_DATABASE_URL` and the
 * sealing key on the machine it ran on. The route sits behind `PlatformOltp`
 * and the admin scope fence (an out-of-scope org answers 404) and is audited
 * server-side (`oltp.migrations.applied`). It applies the workspace's newest
 * successfully compiled revision — never a local checkout.
 *
 * Exit codes: 8 without `--yes`, nothing sent (DDL runs as the owner and no
 * later call undoes it); 9 when a migration ran and failed, `--json` included
 * (the server answers 200 with `ok: false` and the file); 6 for a request the
 * server refused (two workspaces and no `--workspace`, no successful compile);
 * 7 when another apply holds the tenant's lock, the database would not open
 * a session, or the apply ran past the server's budget — retry; a re-run
 * resumes at the first file the database has not run. A budget spent mid-apply
 * is a 200 with `timed_out: true` naming the files that landed; one spent
 * before any DDL is a 504.
 */

import type { Context } from "../context/resolve.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, refusal, usageError } from "../util/errors.js";
import { orgOrHint, resolveOrgId } from "./assume.js";
import {
  call,
  orgPath,
  PROVISION_TIMEOUT_MS,
  type RefusalReason,
  refusalReason
} from "./oltp-client.js";

/** `oxy_oltp::api::apply::ApplyResponse`. */
export interface ApplyResponse {
  org_id: string;
  workspace_id: string;
  revision_id: string;
  applied: string[];
  already_applied: number;
  failed: { file_path: string; message: string } | null;
  /**
   * The server's budget stopped the apply part way: `applied` is what landed,
   * and a re-run continues from there. Absent from a server that predates it.
   */
  timed_out?: boolean;
  ok: boolean;
}

export const applyPath = (orgId: string) => `${orgPath(orgId)}/apply`;

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** The OLTP console's refusals, plus the apply's own. */
function applyRefusal(base: RefusalReason): RefusalReason {
  return (status) => {
    switch (status) {
      case 400:
        return "the org has more than one workspace — pass --workspace <uuid> (the server lists them)";
      case 404:
        return (
          "no OLTP database, workspace or org here — or an org outside your staff grant's scope, " +
          "which answers 404 on purpose"
        );
      case 409:
        return "the workspace has no successful compile to apply — push to its default branch first";
      case 502:
        return "the tenant database would not open a session, so nothing ran — retry";
      case 503:
        return "another apply holds this org's lock — wait for it to finish and retry";
      case 408:
      case 504:
        return (
          "the apply was cut off before it finished; files it completed stay applied and " +
          "recorded — re-run it to continue from the first one the database has not run"
        );
      default:
        return base(status);
    }
  };
}

/** What the Rust command printed, from the server's answer. */
export function renderApply(a: ApplyResponse): string {
  const lines: string[] = [];
  if (a.applied.length === 0 && a.ok) {
    lines.push(`already up to date (${a.already_applied} applied previously)`);
  } else if (a.applied.length > 0) {
    lines.push(`applied ${a.applied.length}:`);
    for (const f of a.applied) lines.push(`    ${f}`);
  }
  if (a.failed) {
    lines.push(`failed: ${a.failed.file_path}`);
    lines.push(`    ${a.failed.message}`);
  }
  if (a.timed_out) {
    lines.push("stopped: the server's time budget ran out before the rest — re-run to continue");
  }
  lines.push(`revision ${a.revision_id} of workspace ${a.workspace_id}`);
  return lines.join("\n");
}

/** The error a not-ok answer exits with: 7 for a cut-off apply, 9 for a failed migration. */
export function applyFailure(result: ApplyResponse): CliError | undefined {
  if (result.timed_out) {
    return new CliError("the apply was cut off before it finished", {
      code: ExitCode.UNAVAILABLE,
      detail:
        `${result.applied.length} file(s) applied in this call and stay applied; ` +
        "re-run it to continue from the first one the database has not run"
    });
  }
  if (result.ok) return undefined;
  return new CliError(
    `migration ${result.failed?.file_path ?? "(unnamed)"} failed against the org's database`,
    {
      code: ExitCode.CHECK_FAILED,
      detail:
        `${result.applied.length} file(s) before it applied in this call and stay applied; ` +
        `nothing after it ran. ${result.failed?.message ?? ""}`
    }
  );
}

/**
 * Apply the org's pending schema migrations. Always needs `--yes`: the DDL runs
 * as the tenant's owner against live business tables, and nothing undoes it.
 */
export async function runOltpApply(
  ctx: Context,
  org: string | undefined,
  flags: { workspace?: string; yes?: boolean; json?: boolean }
): Promise<void> {
  // EVERY USAGE ERROR BEFORE A REQUEST.
  const workspace = flags.workspace?.trim();
  if (workspace !== undefined && !UUID.test(workspace)) {
    throw usageError(`--workspace ${flags.workspace} is not a UUID`);
  }
  const orgId = await resolveOrgId(ctx, org);
  process.stderr.write(
    `On ${ctx.target()}, org ${orgOrHint(ctx, org)}: apply the ` +
      `${workspace ? `workspace ${workspace}'s` : "workspace's"} compiled schemas/*.sql ` +
      "that the database has not run\n" +
      "  the DDL runs as the database owner, and no later call undoes it\n"
  );
  if (!flags.yes) {
    const ws = workspace ? ` --workspace ${workspace}` : "";
    throw refusal("apply needs --yes", {
      detail: "schema migrations change a live database and cannot be rolled back from here",
      remedy: `re-run with --yes: oxyc oltp apply --org ${orgOrHint(ctx, org)}${ws} --yes`
    });
  }

  const result = await call<ApplyResponse>(ctx, "POST", applyPath(orgId), {
    // Always a JSON body: the route's body is all-optional, never absent.
    body: workspace ? { workspace_id: workspace } : {},
    refusal: applyRefusal(refusalReason),
    // A migration can take a while, and it may wake a suspended compute.
    timeoutMs: PROVISION_TIMEOUT_MS
  });
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  } else {
    const text = renderApply(result);
    process.stdout.write(`${result.ok ? out.green(text) : text}\n`);
  }
  const failure = applyFailure(result);
  if (failure) throw failure;
}
