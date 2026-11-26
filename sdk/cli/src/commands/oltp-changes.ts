/**
 * `oxyc oltp changes resync <app>` — re-baseline an app's captured tables in
 * the OLTP change stream, and remove the holds that resolves.
 *
 *   POST /api/customer-apps/{id}/oltp-changes/resync   {table?, environment}
 *
 * The unshipped log rows of a held table are discarded (its new baseline
 * supersedes them): each is printed with its row count. A writer held past
 * the relay's ceiling is baselined whole, whichever table is named. Staff
 * only; audited on the server.
 *
 * Exit codes: 8 without `--yes`, nothing sent; 2 for an environment other
 * than production, nothing sent; a refusal exits by its status (409: the
 * org's `oltp_changes` flag is off, or a generation bump is re-stamping;
 * 404: no capture, or a table not captured).
 */

import { parseJson, request } from "../api/request.js";
import { CUSTOMER_APPS_API, ensureOk, resolveApp, staffCreds } from "../apps/resolve.js";
import type { Context } from "../context/resolve.js";
import { refusal, usageError } from "../util/errors.js";

/** `oxy_api_custom_apps::oltp_changes::resync::ResyncReport`. */
export interface ResyncReport {
  writer: string;
  requested: string[];
  /** A table's name, or `""` for the writer's ceiling hold. */
  released: string[];
  discarded: { table: string; rows: number }[];
}

/** The report, as the human output prints it. */
export function renderResync(report: ResyncReport): string {
  const list = (items: string[]) => (items.length > 0 ? items.join(", ") : "none");
  const released = report.released.map((t) => t || "(the writer's ceiling hold)");
  const lines = [
    `writer ${report.writer}`,
    `requested a baseline of: ${list(report.requested)}`,
    `released holds: ${list(released)}`
  ];
  if (report.discarded.length === 0) {
    lines.push("discarded: no rows");
  } else {
    lines.push("discarded unshipped rows:");
    for (const d of report.discarded) lines.push(`    ${d.table}: ${d.rows}`);
  }
  return lines.join("\n");
}

/** Resync one table, or every captured table of `app`. Always needs `--yes`: discarded rows are gone. */
export async function runOltpChangesResync(
  ctx: Context,
  app: string,
  flags: { table?: string; env?: string; yes?: boolean; json?: boolean }
): Promise<void> {
  const environment = flags.env ?? "production";
  if (environment !== "production") {
    throw usageError(
      `--app-env ${environment} has no change stream`,
      "only production's store captures changes"
    );
  }
  const table = flags.table?.trim() || undefined;
  if (!flags.yes) {
    throw refusal("resync needs --yes", {
      detail:
        "the unshipped log rows of a held table are discarded, and a writer held past the " +
        "relay's ceiling loses every waiting row; a new baseline replaces them",
      remedy: `re-run with --yes: oxyc oltp changes resync ${app}${table ? ` --table ${table}` : ""} --yes`
    });
  }
  const creds = staffCreds(ctx);
  const { appId, label } = await resolveApp(creds, app);
  const response = await request({
    target: creds.target,
    path: `${CUSTOMER_APPS_API}/${appId}/oltp-changes/resync`,
    method: "POST",
    bearer: creds.bearer,
    headers: creds.headers,
    body: JSON.stringify({ table, environment })
  });
  ensureOk(response, creds);
  const report = parseJson(response.body) as ResyncReport;
  if (flags.json) {
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
    return;
  }
  process.stdout.write(`${label}\n${renderResync(report)}\n`);
}
