/**
 * `oxyc env diff <app> <name>` — what an environment's planes differ from
 * production by. Today that is the OLTP schema of a sandbox that runs on a
 * branch of its own (`--plane oltp`): the tables and columns its migrations
 * added, removed or changed, so a person can read them before promoting.
 *
 * `GET …/environments/{name}/diff` answers `{environment, planes: {oltp?}}`
 * and is admitted through `show`'s door. An environment with no branch of its
 * own answers `planes: {}`; a branch that is not ready (not cut, being
 * migrated, failed) is `409 oltp_branch_unready`, whose message names the fix.
 */

import { parseJson, request } from "../api/request.js";
import { parseAppEnv } from "../apps/environment.js";
import { ensureOk, resolveApp, staffCreds } from "../apps/resolve.js";
import type { Context } from "../context/resolve.js";
import { usageError } from "../util/errors.js";
import { environmentsPath } from "./env.js";

/** The planes `env diff` can show. Airhouse joins when it grows a branch. */
export const DIFF_PLANES: readonly string[] = ["oltp"];

export interface OltpDiff {
  /** The app's writer schema, which both sides were read in. */
  schema: string;
  tables_added: string[];
  tables_removed: string[];
  /** `table.column type`. */
  columns_added: string[];
  columns_removed: string[];
  /** `table.column: <production> → <branch>`. */
  columns_changed: string[];
}

export interface EnvDiff {
  environment: string;
  planes: { oltp?: OltpDiff };
}

/** `--plane` values to the planes to show; none given means every plane. */
export function parseDiffPlanes(values: readonly string[]): string[] {
  const keys = values.flatMap((v) => v.split(",")).map((k) => k.trim());
  const filtered = keys.filter((k) => k !== "");
  for (const key of filtered) {
    if (!DIFF_PLANES.includes(key)) {
      throw usageError(`${key} is not a plane with a diff`, `pick from: ${DIFF_PLANES.join(", ")}`);
    }
  }
  return filtered;
}

/** Fetch the diff; `planes` limits it to those plane keys. No printing. */
export async function envDiff(
  ctx: Context,
  app: string,
  name: string,
  planes: readonly string[] = []
): Promise<EnvDiff> {
  const environment = parseAppEnv(name);
  const wanted = parseDiffPlanes(planes);
  const creds = staffCreds(ctx);
  const { appId } = await resolveApp(creds, app);
  const response = await request({
    target: creds.target,
    path: `${environmentsPath(appId, environment)}/diff`,
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  const diff = parseJson(response.body) as EnvDiff;
  if (wanted.length === 0) return diff;
  const kept = Object.entries(diff.planes).filter(([plane]) => wanted.includes(plane));
  return { ...diff, planes: Object.fromEntries(kept) };
}

/** The human lines for one plane's diff: `+` added, `-` removed, `~` changed. */
export function describeOltpDiff(environment: string, diff: OltpDiff): string[] {
  const lines = [
    ...diff.tables_added.map((t) => `+ table ${t}`),
    ...diff.tables_removed.map((t) => `- table ${t}`),
    ...diff.columns_added.map((c) => `+ ${c}`),
    ...diff.columns_removed.map((c) => `- ${c}`),
    ...diff.columns_changed.map((c) => `~ ${c}`)
  ];
  const heading = `${environment} — oltp, schema ${diff.schema}, against production`;
  return lines.length === 0
    ? [heading, "  no difference"]
    : [heading, ...lines.map((l) => `  ${l}`)];
}

export function describeEnvDiff(diff: EnvDiff): string[] {
  if (diff.planes.oltp === undefined) {
    return [
      `${diff.environment} has no plane on a branch of its own, so it has no difference to show`
    ];
  }
  return describeOltpDiff(diff.environment, diff.planes.oltp);
}

export async function runEnvDiff(
  ctx: Context,
  app: string,
  name: string,
  opts: { plane?: string[]; json: boolean }
): Promise<void> {
  const diff = await envDiff(ctx, app, name, opts.plane ?? []);
  if (opts.json) {
    process.stdout.write(`${JSON.stringify(diff)}\n`);
    return;
  }
  process.stdout.write(`${describeEnvDiff(diff).join("\n")}\n`);
}
