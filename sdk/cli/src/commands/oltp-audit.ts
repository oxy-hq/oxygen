/**
 * `oxyc oltp audit | rotate` — the confinement audit of an org's OLTP database,
 * and a writer's password rotation.
 *
 *   POST /api/admin/orgs/{org_id}/oltp/audit                  no body; every role's real authority
 *   POST /api/admin/orgs/{org_id}/oltp/roles/{role}/rotate    no body; re-mints one writer's password
 *
 * These replaced the Rust `oxy oltp audit | rotate`, which read
 * `OXY_DATABASE_URL`, the sealing key and the provider token on the machine they
 * ran on. Both routes sit behind `PlatformOltp` and the admin scope fence (an
 * out-of-scope org answers 404), and both are audited server-side
 * (`oltp.audited`, `oltp.role.rotated`).
 *
 * The audit's verdict is the server's (`clean`): this file renders it and exits
 * 9 when it is false, `--json` included, so a script can gate on `$?` and still
 * read the report. A rotation never returns the password: the new one is
 * sealed server-side, and every app using the writer picks it up from there.
 */

import type { Context } from "../context/resolve.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, refusal, usageError } from "../util/errors.js";
import { orgOrHint, resolveOrgId } from "./assume.js";
import { parseWriter } from "./oltp.js";
import {
  call,
  orgPath,
  PROVISION_TIMEOUT_MS,
  type RefusalReason,
  refusalReason
} from "./oltp-client.js";

/** `oxy_oltp::confinement_audit::RoleConfinement`. */
export interface RoleConfinement {
  role: string;
  login: boolean;
  /** `admin` (the owner), `none` (confined), else the attributes joined by `+`. */
  risk: string;
  attributes: string[];
  memberships: string[];
  confined: boolean;
}

/** `oxy_oltp::confinement_audit::ConfinementAudit`. */
export interface ConfinementAudit {
  org_id: string;
  database: string;
  owner_role: string;
  roles: RoleConfinement[];
  grants: Array<{ role: string; findings: string[] }>;
  unconfined: number;
  drifted: number;
  clean: boolean;
}

/** `oxy_api_data_plane::oltp::confinement::RotatedRole` — never a password or a DSN. */
export interface RotatedRole {
  writer: string;
  role: string;
  schema: string;
}

export const auditPath = (orgId: string) => `${orgPath(orgId)}/audit`;
export const rotatePath = (orgId: string, writer: string) =>
  `${orgPath(orgId)}/roles/${encodeURIComponent(writer)}/rotate`;

/** The OLTP console's refusals, plus the audit's own 404 and 502. */
function auditRefusal(base: RefusalReason): RefusalReason {
  return (status) => {
    if (status === 404) {
      return (
        "this org has no OLTP database to audit — or it sits outside your staff grant's scope, " +
        "which answers 404 on purpose"
      );
    }
    if (status === 502) {
      return "the tenant database would not open a session, so the audit did not run — retry";
    }
    return base(status);
  };
}

/** The table and the two summaries the Rust command printed, from the server's verdict. */
export function renderAudit(a: ConfinementAudit): string {
  const lines: string[] = [];
  // The Rust command's fixed columns, so a diff of old and new output is clean.
  const row = (cells: [string, string, string, string]) =>
    `${cells[0].padEnd(28)} ${cells[1].padEnd(7)} ${cells[2].padEnd(6)} ${cells[3]}`;
  lines.push(row(["role", "login", "risk", "memberships"]));
  for (const r of a.roles) {
    lines.push(
      row([
        r.role,
        r.login ? "yes" : "no",
        // Empty when only a membership makes the role unconfined.
        r.risk || "member",
        r.memberships.length > 0 ? r.memberships.join(",") : "-"
      ])
    );
  }
  lines.push("");
  lines.push(
    a.unconfined > 0
      ? `${a.unconfined} role(s) hold authority they should not. Re-run \`oxyc oltp provision\` ` +
          "to re-mint them, or `oxyc oltp rotate --role <r> --yes` for one writer."
      : "all non-admin roles confined"
  );
  lines.push("");
  lines.push("grants outside each role's scope");
  for (const g of a.grants) {
    if (g.findings.length === 0) {
      lines.push(`  ${g.role.padEnd(28)} none`);
      continue;
    }
    lines.push(`  ${g.role.padEnd(28)} ${g.findings.length} finding(s)`);
    for (const f of g.findings) lines.push(`    - ${f}`);
  }
  lines.push("");
  lines.push(
    a.drifted > 0
      ? `${a.drifted} role(s) hold privileges outside their scope. Revoke each one above as the ` +
          "owner; `oxyc oltp provision` refuses a writer that still holds any (OXY03)."
      : "no role holds privileges outside its scope"
  );
  return lines.join("\n");
}

/** The exit-9 message, worded as the Rust command's error was. */
export function auditFailure(a: ConfinementAudit): string | undefined {
  if (a.clean) return undefined;
  if (a.unconfined > 0 && a.drifted > 0) {
    return `${a.unconfined} role(s) are not confined and ${a.drifted} hold privileges outside their scope`;
  }
  if (a.unconfined > 0) return `${a.unconfined} role(s) are not confined`;
  if (a.drifted > 0) return `${a.drifted} role(s) hold privileges outside their scope`;
  // `clean: false` with both counts zero is a server this client does not
  // understand; failing the check is the safe reading.
  return "the server reported the audit as not clean";
}

/**
 * Audit every role in the org's database. Exit 0 when clean, 9 when the check
 * ran and found a violation; the report goes to stdout either way.
 */
export async function runOltpAudit(
  ctx: Context,
  org: string | undefined,
  flags: { json?: boolean }
): Promise<void> {
  const orgId = await resolveOrgId(ctx, org);
  // A POST although it changes nothing: it wakes a suspended compute and
  // writes an audit row, so it is an act, not a read (decided 2026-11-21).
  const audit = await call<ConfinementAudit>(ctx, "POST", auditPath(orgId), {
    refusal: auditRefusal(refusalReason),
    // It opens a session as the owner, which may wake a suspended compute.
    timeoutMs: PROVISION_TIMEOUT_MS
  });
  process.stdout.write(
    flags.json ? `${JSON.stringify(audit, null, 2)}\n` : `${renderAudit(audit)}\n`
  );
  const failed = auditFailure(audit);
  if (failed !== undefined) {
    throw new CliError(failed, {
      code: ExitCode.CHECK_FAILED,
      detail: `org ${orgOrHint(ctx, org)}, database ${audit.database}`
    });
  }
}

/**
 * Rotate one writer's password and reseal it. Always needs `--yes`: the old
 * password stops working at once, so any connection holding it outside Oxy (a
 * `psql` from `oxyc oltp dsn`, say) is cut off, and nothing brings it back.
 */
export async function runOltpRotate(
  ctx: Context,
  org: string | undefined,
  flags: { role?: string; yes?: boolean; json?: boolean }
): Promise<void> {
  // EVERY USAGE ERROR BEFORE A REQUEST.
  if (!flags.role) throw usageError("name the writer: --role app:<slug> | pipeline:<source>");
  if (flags.role.trim() === "analyst" || flags.role.trim() === "owner") {
    throw usageError(
      `--role ${flags.role.trim()} has no rotation of its own`,
      "only a writer rotates; the analyst and the owner are re-minted by `oxyc oltp provision`"
    );
  }
  const writer = parseWriter(flags.role);
  const orgId = await resolveOrgId(ctx, org);
  process.stderr.write(
    `On ${ctx.target()}, org ${orgOrHint(ctx, org)}: rotate ${writer.spec} (schema ${writer.schema})\n` +
      "  the old password stops working at once; the new one is sealed and never shown\n"
  );
  if (!flags.yes) {
    throw refusal("rotate needs --yes", {
      detail: "a rotation cuts off every session still holding the old password",
      remedy: `re-run with --yes: oxyc oltp rotate --org ${orgOrHint(ctx, org)} --role ${writer.spec} --yes`
    });
  }

  const rotated = await call<RotatedRole>(ctx, "POST", rotatePath(orgId, writer.spec), {
    timeoutMs: PROVISION_TIMEOUT_MS
  });
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(rotated, null, 2)}\n`);
    return;
  }
  process.stdout.write(`${out.green(`rotated ${rotated.role} (${rotated.schema})`)}\n`);
}
