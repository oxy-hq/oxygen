/**
 * `oxyc oltp status | provision | reset` — an org's OLTP database, from a laptop.
 *
 * A thin client over the admin console's routes, so staff can see and request
 * provisioning without a shell on the server. The Rust `oxy oltp` reads
 * `OXY_DATABASE_URL` and has no `--env`: it only reaches the control plane of
 * the machine it runs on.
 *
 *   GET  /api/admin/oltp                                  every org, with a database or without
 *   GET  /api/admin/orgs/{org_id}/oltp                    one org's store and writers
 *   POST /api/admin/orgs/{org_id}/oltp/provision          {"writers": [...], "branch"?: "staging"}
 *   GET  /api/admin/orgs/{org_id}/oltp/branches/staging   the staging branch (`oltp-branch.ts`)
 *   POST /api/admin/orgs/{org_id}/oltp/branches/staging/reset  {"confirm": true}
 *
 * All of them sit behind the `PlatformOltp` capability and the admin scope
 * fence, so an org outside a bounded staff grant answers 404 rather than 403.
 * None of them returns a credential.
 */

import type { Context } from "../context/resolve.js";
import {
  appWriterName,
  isValidWriterName,
  WRITER_NAME_RULE,
  WRITER_NAME_SHAPE
} from "../publish/manifest.js";
import * as log from "../ui/log.js";
import { table } from "../ui/render.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { orgOrHint, resolveOrgId } from "./assume.js";
import {
  type BranchName,
  type BranchStatus,
  branchLines,
  fetchBranch,
  parseBranch
} from "./oltp-branch.js";
import { call, confirm, orgPath, PROVISION_TIMEOUT_MS } from "./oltp-client.js";

/** One row of `GET /admin/oltp` — `oxy_oltp::api::admin::TenantRow`. */
interface TenantRow {
  org_id: string;
  org_name: string;
  database: string;
  host: string;
  provider: string;
  region: string;
  /** `none` for an org with no database. */
  status: string;
  schemas: Array<{ schema: string; kind: string; analytics_visible: boolean }>;
  analyst_ready: boolean;
  platform_drift: boolean;
}

/** `oxy_oltp::api::handlers::SchemaInfo`. */
interface SchemaInfo {
  schema: string;
  kind: string;
  writer_name: string;
  role: string;
  analytics_visible: boolean;
}

/** `oxy_oltp::api::handlers::ConnectionInfoResponse` — metadata, never a credential. */
interface ConnectionInfo {
  is_provisioned: boolean;
  host: string;
  database: string;
  provider: string;
  project_name: string;
  console_url?: string | null;
  region: string;
  status: string;
  analyst_role: string;
  analyst_ready: boolean;
  platform_schema_version: number;
  expected_platform_schema_version: number;
  schemas: SchemaInfo[];
}

/** A writer as the server takes it, and as it was typed. */
export interface WriterPlan {
  /** On the wire: `app:<writer>` or `pipeline:<source>`. */
  spec: string;
  typed: string;
  schema: string;
}

/**
 * `--writer app:<slug>` / `pipeline:<source>` → what the server accepts.
 *
 * The server takes a WRITER NAME, and a slug's hyphens are illegal in one, so
 * `app:store-ops` is derived to `app:store_ops` exactly as the platform derives
 * `ctx.oltp`'s schema. A name already carrying `_` is what the server's own
 * messages print, and is validated as the writer it already is.
 */
export function parseWriter(typed: string): WriterPlan {
  const colon = typed.indexOf(":");
  const kind = colon < 0 ? "" : typed.slice(0, colon);
  const name = colon < 0 ? "" : typed.slice(colon + 1).trim();
  if (kind === "app") {
    const writer = name.includes("_")
      ? isValidWriterName(name)
        ? name
        : undefined
      : appWriterName(name);
    if (writer === undefined) {
      throw usageError(
        `invalid app writer in --writer ${typed}`,
        `from a slug, ${WRITER_NAME_RULE}`
      );
    }
    return { spec: `app:${writer}`, typed, schema: `app_${writer}` };
  }
  if (kind === "pipeline") {
    if (!isValidWriterName(name)) {
      throw usageError(
        `invalid pipeline source in --writer ${typed}`,
        `it must be ${WRITER_NAME_SHAPE}`
      );
    }
    return { spec: `pipeline:${name}`, typed, schema: `raw_${name}` };
  }
  throw usageError(`--writer ${typed} must look like app:<slug> or pipeline:<source>`);
}

export async function runOltpStatus(
  ctx: Context,
  org: string | undefined,
  json: boolean
): Promise<void> {
  const named = orgOrHint(ctx, org);
  if (!named) return listTenants(ctx, json);
  const orgId = await resolveOrgId(ctx, org);
  const info = await call<ConnectionInfo>(ctx, "GET", orgPath(orgId));
  // No database, no branch — so no second request to say so.
  const staging = info.is_provisioned ? await stagingOrNothing(ctx, orgId) : undefined;
  if (json) {
    const doc = staging === undefined ? info : { ...info, staging_branch: staging };
    process.stdout.write(`${JSON.stringify(doc, null, 2)}\n`);
    return;
  }
  printStore(named, info);
  if (staging !== undefined) process.stdout.write(`\n${branchLines(named, staging).join("\n")}\n`);
}

/**
 * The staging branch's status, or nothing from a deployment that predates the
 * route — a 404 here cannot be the scope fence, which just let the store read
 * through for the same org.
 */
async function stagingOrNothing(ctx: Context, orgId: string): Promise<BranchStatus | undefined> {
  try {
    return await fetchBranch(ctx, orgId, "staging");
  } catch (e) {
    if (e instanceof CliError && e.code === ExitCode.NOT_FOUND) return undefined;
    throw e;
  }
}

async function listTenants(ctx: Context, json: boolean): Promise<void> {
  const rows = await call<TenantRow[]>(ctx, "GET", "/api/admin/oltp");
  if (!Array.isArray(rows)) {
    throw new CliError("GET /api/admin/oltp did not return a list", { code: ExitCode.FAILURE });
  }
  if (json) {
    process.stdout.write(`${JSON.stringify(rows, null, 2)}\n`);
    return;
  }
  if (rows.length === 0) {
    log.info(`no organizations are visible to you on ${ctx.target()}`);
    return;
  }
  process.stdout.write(
    `${table(rows, [
      { header: "ORG", value: (r) => r.org_name },
      { header: "ORG ID", value: (r) => r.org_id },
      { header: "STATUS", value: (r) => r.status },
      { header: "DATABASE", value: (r) => r.database },
      { header: "PROVIDER", value: (r) => [r.provider, r.region].filter(Boolean).join(" / ") },
      {
        header: "SCHEMAS",
        value: (r) =>
          r.schemas
            .map((s) => (s.analytics_visible ? `${s.schema} (analytics)` : s.schema))
            .join(", ")
      },
      { header: "NOTES", value: (r) => tenantNotes(r) }
    ])}\n`
  );
  const provisioned = rows.filter((r) => r.status !== "none").length;
  log.info(`${provisioned} of ${rows.length} org(s) have an OLTP database`);
}

function tenantNotes(row: TenantRow): string {
  if (row.status === "none") return "";
  const notes: string[] = [];
  if (!row.analyst_ready) notes.push("analyst not minted");
  if (row.platform_drift) notes.push("platform objects behind");
  return notes.join(", ");
}

function printStore(org: string, info: ConnectionInfo): void {
  if (!info.is_provisioned) {
    process.stdout.write(`no OLTP database for org ${org}\n`);
    log.hint(`oxyc oltp provision --org ${org} --writer app:<slug>`);
    return;
  }
  const fields: Array<[string, string]> = [
    ["database", info.database],
    ["host", info.host],
    ["provider", [info.provider, info.region].filter(Boolean).join(" / ")],
    ["project", [info.project_name, info.console_url].filter(Boolean).join("  ")],
    ["status", info.status],
    ["platform", `v${info.platform_schema_version}/${info.expected_platform_schema_version}`],
    [
      "analyst",
      info.analyst_ready
        ? `${info.analyst_role} (ready)`
        : `${info.analyst_role} — NOT MINTED, postgres_managed cannot resolve`
    ]
  ];
  process.stdout.write(`${fields.map(([k, v]) => `${k.padEnd(9)} ${v}`).join("\n")}\n`);
  if (info.schemas.length === 0) {
    process.stdout.write("\nno writers yet\n");
    return;
  }
  process.stdout.write(
    `\n${table(info.schemas, [
      { header: "SCHEMA", value: (s) => s.schema },
      { header: "KIND", value: (s) => s.kind },
      { header: "WRITER", value: (s) => s.writer_name },
      { header: "ROLE", value: (s) => s.role },
      { header: "ANALYTICS", value: (s) => (s.analytics_visible ? "visible" : "hidden") }
    ])}\n`
  );
}

export async function runOltpProvision(
  ctx: Context,
  org: string | undefined,
  typedWriters: string[],
  flags: { yes?: boolean; json?: boolean; branch?: string }
): Promise<void> {
  // EVERY USAGE ERROR BEFORE A REQUEST. The server also parses writers before it
  // provisions, but a typo should not first cost an org lookup and a login.
  const branch = parseBranch(flags.branch);
  if (typedWriters.length === 0 && branch === undefined) {
    throw usageError(
      "name at least one --writer, or --branch staging",
      "app:<slug> for a custom app's ctx.oltp store, pipeline:<source> for an Airway pipeline"
    );
  }
  const writers = new Map<string, WriterPlan>();
  for (const plan of typedWriters.map(parseWriter)) {
    if (!writers.has(plan.spec)) writers.set(plan.spec, plan);
  }

  const orgId = await resolveOrgId(ctx, org);
  const named = orgOrHint(ctx, org);
  const current = await call<ConnectionInfo>(ctx, "GET", orgPath(orgId));
  const currentBranch =
    branch !== undefined && current.is_provisioned
      ? await fetchBranch(ctx, orgId, branch)
      : undefined;
  printPlan(ctx.target(), named, orgId, current, [...writers.values()]);
  if (branch !== undefined) printBranchPlan(branch, currentBranch);
  if (!flags.yes) {
    await confirm("Provision?", {
      verb: "provisioning",
      why: "it creates billable resources at the deployment's OLTP provider",
      declined: "not provisioned — the confirmation was declined"
    });
  }

  // `branch` only when asked for: the body an ordinary provision sends is
  // exactly what it always was.
  const specs = [...writers.keys()];
  const info = await call<ConnectionInfo>(ctx, "POST", `${orgPath(orgId)}/provision`, {
    body: branch === undefined ? { writers: specs } : { writers: specs, branch },
    timeoutMs: PROVISION_TIMEOUT_MS
  });
  const staging = branch === undefined ? undefined : await fetchBranch(ctx, orgId, branch);
  if (flags.json) {
    const doc = staging === undefined ? info : { ...info, staging_branch: staging };
    process.stdout.write(`${JSON.stringify(doc, null, 2)}\n`);
    return;
  }
  process.stdout.write(`${out.green(`provisioned ${info.database} on ${info.host}`)}\n`);
  printStore(named, info);
  if (staging !== undefined) process.stdout.write(`\n${branchLines(named, staging).join("\n")}\n`);
}

/** The branch's line of the plan: new (and whom it serves) or reconciled. */
function printBranchPlan(branch: BranchName, current: BranchStatus | undefined): void {
  const line = current?.provisioned
    ? `  branch    ${branch}: ${current.database} on ${current.host} — exists, reconciled ` +
      "(writers added since the cut get their schema there; nobody's data is reset)"
    : `  branch    ${branch}: a NEW copy of the database, cut from production now — staging ` +
      "for every app in the org, and a billable compute on a managed provider";
  process.stderr.write(`${line}\n`);
}

/**
 * What is about to happen, on stderr — stdout is the result — and not through
 * `log.info`, which `--quiet` silences: this is what the prompt asks about.
 */
function printPlan(
  target: string,
  org: string,
  orgId: string,
  current: ConnectionInfo,
  writers: WriterPlan[]
): void {
  const existing = new Set(current.schemas.map((s) => s.schema));
  const lines = [`On ${target}, for org ${org === orgId ? orgId : `${org} (${orgId})`}:`];
  lines.push(
    current.is_provisioned
      ? `  database  ${current.database} on ${current.host} — exists, reconciled`
      : "  database  a NEW database at the deployment's OLTP provider — a billable resource"
  );
  for (const w of writers) {
    const from = w.typed === w.spec ? "" : ` (from ${w.typed})`;
    const state = existing.has(w.schema) ? "exists, reconciled" : "new schema and read-write role";
    lines.push(`  writer    ${w.spec} → ${w.schema}${from} — ${state}`);
  }
  process.stderr.write(`${lines.join("\n")}\n`);
}
