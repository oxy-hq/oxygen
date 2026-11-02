/**
 * `oxyc airhouse status | provision` — a workspace's Airhouse tenant, from a laptop.
 *
 * A thin client over the admin console's routes, as `oxyc oltp` is over OLTP's.
 * The server provisions a workspace's tenant itself; `provision` is the repair
 * and backfill tool — a workspace that predates that, or whose tenant or service
 * account did not come up.
 *
 *   GET  /api/admin/airhouse                                    every workspace, with a tenant or without
 *   POST /api/admin/workspaces/{workspace_id}/airhouse/provision    no body; idempotent
 *
 * Both sit behind the `PlatformAirhouse` capability and the admin scope fence,
 * so a workspace outside a bounded staff grant answers 404 rather than 403.
 * Neither returns a credential. There is no route that reads one workspace's
 * tenant, so `status --workspace` reads the fleet and picks the row out.
 */

import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { table } from "../ui/render.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { call, confirm, PROVISION_TIMEOUT_MS } from "./oltp-client.js";

/** One row of `GET /admin/airhouse` — `airhouse::api::admin::AirhouseFleetRow`. */
export interface AirhouseFleetRow {
  workspace_id: string;
  workspace_name: string;
  org_id: string | null;
  org_name: string;
  /** `none` when the workspace has no tenant yet. */
  status: string;
  tenant_id: string;
  bucket: string;
  prefix: string;
  /** False means the workspace cannot mint the credentials every query uses. */
  service_account_ready: boolean;
  sa_rotated_at: string | null;
  created_at: string | null;
  service_account_id: string | null;
  sa_created_at: string | null;
  bearer_max_role: string | null;
  bearer_max_ttl_secs: number | null;
}

/** `airhouse::api::admin::AirhouseFleet`. */
interface AirhouseFleet {
  rows: AirhouseFleetRow[];
  truncated: { unprovisioned: boolean; provisioned: boolean };
}

const FLEET_PATH = "/api/admin/airhouse";

export const provisionPath = (workspaceId: string) =>
  `/api/admin/workspaces/${encodeURIComponent(workspaceId)}/airhouse/provision`;

/** The tenant name the server derives — `tenant_name_for_workspace` in `crates/airhouse`. */
export const tenantNameFor = (workspaceId: string) => `oxy-ws-${workspaceId}`;

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** Why the Airhouse console refused, in the terms an operator acts on. */
function refusalReason(status: number): string | undefined {
  switch (status) {
    case 400:
      return "the server rejected the request as malformed";
    case 401:
      return "not authenticated for this target";
    case 403:
      return (
        "the Airhouse console needs platform staff standing that may operate it — and an active " +
        "`oxyc assume` session closes /admin"
      );
    case 404:
      return "no such workspace here — or one outside your staff grant's scope, which answers 404 on purpose";
    case 409:
      return "the tenant name is already taken at Airhouse — an operator has to resolve it";
    case 422:
      return "Airhouse refused the tenant name";
    case 502:
      return "the Airhouse API failed the provision — it is idempotent, so retrying is safe";
    case 503:
      return "this deployment has no Airhouse configured — not an Oxy fault";
    default:
      return undefined;
  }
}

const AIRHOUSE = { refusal: refusalReason, routesArea: "airhouse" } as const;

/** `--workspace` must be a workspace id: the route takes a UUID and nothing else. */
function parseWorkspace(workspace: string | undefined, verb: string): string | undefined {
  if (workspace === undefined) return undefined;
  const id = workspace.trim();
  if (!UUID.test(id)) {
    throw usageError(
      `--workspace ${workspace} is not a workspace id`,
      `${verb} takes the workspace's UUID — \`oxyc airhouse status\` lists them`
    );
  }
  return id.toLowerCase();
}

async function fetchFleet(ctx: Context): Promise<AirhouseFleet> {
  const fleet = await call<AirhouseFleet>(ctx, "GET", FLEET_PATH, AIRHOUSE);
  if (!Array.isArray(fleet?.rows)) {
    throw new CliError(`GET ${FLEET_PATH} did not return a fleet`, { code: ExitCode.FAILURE });
  }
  return fleet;
}

const hasTenant = (row: AirhouseFleetRow) => row.status !== "none";

export async function runAirhouseStatus(
  ctx: Context,
  workspace: string | undefined,
  json: boolean
): Promise<void> {
  const workspaceId = parseWorkspace(workspace, "status --workspace");
  const fleet = await fetchFleet(ctx);
  if (workspaceId === undefined) return printFleet(ctx, fleet, json);

  const row = fleet.rows.find((r) => r.workspace_id.toLowerCase() === workspaceId);
  if (!row) throw missingFromFleet(workspaceId, fleet);
  if (json) {
    process.stdout.write(`${JSON.stringify(row, null, 2)}\n`);
    return;
  }
  printRow(row);
}

/**
 * Absent from the fleet. The read is capped and the cap falls on workspaces
 * WITHOUT a tenant, so a capped page cannot say "no such workspace".
 */
function missingFromFleet(workspaceId: string, fleet: AirhouseFleet): CliError {
  if (fleet.truncated.unprovisioned || fleet.truncated.provisioned) {
    return new CliError(`workspace ${workspaceId} is not on the fleet page`, {
      code: ExitCode.NOT_FOUND,
      detail:
        "the fleet read is capped and this one was cut, so the workspace may exist without a tenant",
      hint: `oxyc airhouse provision --workspace ${workspaceId} is idempotent and answers for one workspace`
    });
  }
  return new CliError(`workspace ${workspaceId} is not on the fleet`, {
    code: ExitCode.NOT_FOUND,
    detail: "no such workspace here — or one outside your staff grant's scope"
  });
}

function printFleet(ctx: Context, fleet: AirhouseFleet, json: boolean): void {
  if (json) {
    process.stdout.write(`${JSON.stringify(fleet, null, 2)}\n`);
    return;
  }
  const rows = fleet.rows;
  if (rows.length === 0) {
    log.info(`no workspaces are visible to you on ${ctx.target()}`);
    return;
  }
  process.stdout.write(
    `${table(rows, [
      { header: "WORKSPACE", value: (r) => r.workspace_name },
      { header: "WORKSPACE ID", value: (r) => r.workspace_id },
      { header: "ORG", value: (r) => r.org_name },
      { header: "STATUS", value: (r) => r.status },
      { header: "TENANT", value: (r) => r.tenant_id },
      {
        header: "NOTES",
        value: (r) => (hasTenant(r) && !r.service_account_ready ? "no service account" : "")
      }
    ])}\n`
  );
  const provisioned = rows.filter(hasTenant).length;
  log.info(`${provisioned} of ${rows.length} workspace(s) have an Airhouse tenant`);
  if (fleet.truncated.provisioned) {
    log.warn("the fleet hit its cap on provisioned workspaces — a tenant may be missing here");
  } else if (fleet.truncated.unprovisioned) {
    log.warn("the fleet was capped: every tenant is listed, but not every workspace without one");
  }
}

function printRow(row: AirhouseFleetRow): void {
  const where = `${row.workspace_name} (${row.workspace_id})`;
  if (!hasTenant(row)) {
    process.stdout.write(`no Airhouse tenant for workspace ${where}\n`);
    log.hint(`oxyc airhouse provision --workspace ${row.workspace_id}`);
    return;
  }
  const fields: Array<[string, string]> = [
    ["workspace", where],
    ["org", row.org_name || "(none)"],
    ["status", row.status],
    ["tenant", row.tenant_id],
    ["storage", `s3://${row.bucket}/${row.prefix}`],
    [
      "service",
      row.service_account_ready
        ? `ready (${row.service_account_id ?? "unknown id"})`
        : "NOT BOUND — the workspace cannot mint credentials; provision again to reconcile"
    ],
    ["created", row.created_at ?? "unknown"]
  ];
  process.stdout.write(`${fields.map(([k, v]) => `${k.padEnd(9)} ${v}`).join("\n")}\n`);
}

export async function runAirhouseProvision(
  ctx: Context,
  workspace: string | undefined,
  flags: { yes?: boolean; json?: boolean }
): Promise<void> {
  // EVERY USAGE ERROR BEFORE A REQUEST.
  const workspaceId = parseWorkspace(workspace, "provision --workspace");
  if (workspaceId === undefined) {
    throw usageError(
      "name the workspace: --workspace <id>",
      "`oxyc airhouse status` lists every workspace and whether it has a tenant"
    );
  }

  const fleet = await fetchFleet(ctx);
  const current = fleet.rows.find((r) => r.workspace_id.toLowerCase() === workspaceId);
  printPlan(ctx.target(), workspaceId, current);
  if (!flags.yes) {
    await confirm("Provision?", {
      verb: "provisioning",
      why: "it creates a tenant and a service account at the deployment's Airhouse, under a global name that cannot be reused",
      declined: "not provisioned — the confirmation was declined"
    });
  }

  const row = await call<AirhouseFleetRow>(ctx, "POST", provisionPath(workspaceId), {
    ...AIRHOUSE,
    timeoutMs: PROVISION_TIMEOUT_MS
  });
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(row, null, 2)}\n`);
    return;
  }
  const verb = current && hasTenant(current) ? "reconciled" : "provisioned";
  process.stdout.write(`${out.green(`${verb} Airhouse tenant ${row.tenant_id}`)}\n`);
  printRow(row);
}

/**
 * What is about to happen, on stderr — stdout is the result — and not through
 * `log.info`, which `--quiet` silences: this is what the prompt asks about.
 */
function printPlan(
  target: string,
  workspaceId: string,
  current: AirhouseFleetRow | undefined
): void {
  const lines: string[] = [];
  if (!current) {
    lines.push(`On ${target}, for workspace ${workspaceId}:`);
    lines.push(
      `  tenant    not on the fleet page — if the workspace has none, a NEW tenant ${tenantNameFor(workspaceId)}`
    );
  } else {
    const org = current.org_name ? `, org ${current.org_name}` : "";
    lines.push(
      `On ${target}, for workspace ${current.workspace_name} (${current.workspace_id})${org}:`
    );
    lines.push(
      hasTenant(current)
        ? `  tenant    ${current.tenant_id} — exists (${current.status}), reconciled`
        : `  tenant    a NEW tenant ${tenantNameFor(workspaceId)} — the name is global and permanent`
    );
    if (hasTenant(current) && !current.service_account_ready) {
      lines.push("  service   not bound — provisioning binds it");
    }
  }
  process.stderr.write(`${lines.join("\n")}\n`);
}
