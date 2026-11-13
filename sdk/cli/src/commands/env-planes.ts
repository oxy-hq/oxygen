/**
 * `oxyc env update` and `oxyc env reset` — change which planes of an
 * environment are isolated, and put one back to a clean copy of its parent
 * (`internal-docs/custom-app-sandboxes.md` §6).
 *
 * `PUT …/environments/{name}/planes` REPLACES the stored `planes` object, so
 * `update` reads the environment (`planes_set`), merges the flags into it and
 * sends the result; `--replace` sends the flags alone. `POST …/reset` takes
 * the plane keys. Both use the delete door: whoever may open the environment,
 * and a sandbox agent token only for a sandbox it created. Production has no
 * planes, which the server answers `409 production_has_no_planes`.
 */

import { parseJson, request } from "../api/request.js";
import { parseAppEnv } from "../apps/environment.js";
import { type Creds, ensureOk, resolveApp, staffCreds } from "../apps/resolve.js";
import { isSandboxAgentToken } from "../auth/token-kind.js";
import type { Context } from "../context/resolve.js";
import { usageError } from "../util/errors.js";
import { type Environment, environmentsPath, fetchEnvironment, printEnvironment } from "./env.js";
import {
  applyDestinations,
  applyFetchHosts,
  type DestinationFlags,
  mergePlanes,
  noDestinationEdits,
  type PlaneValue,
  parseDestinationFlags,
  parseFetchHosts,
  parsePlaneFlags,
  parsePlaneKeys,
  requireForkAt,
  validatePlaneMap
} from "./env-plane-args.js";

/** One plane's outcome from `POST …/reset`. */
export interface PlaneReset {
  plane: string;
  status: "done" | "queued" | string;
  run_id: string | null;
}

export interface EnvReset {
  name: string;
  planes: PlaneReset[];
}

/** A sandbox agent token reaches only the sandboxes it created; say so before a request. */
function refuseOutsideOwnSandbox(creds: Creds, environment: string, verb: string): void {
  if (isSandboxAgentToken(creds.bearer) && !environment.startsWith("dev-")) {
    throw usageError(
      `a sandbox agent token cannot ${verb} ${environment}`,
      "it changes only the dev-<handle> sandboxes it created"
    );
  }
}

/**
 * Set planes on an environment, keeping what is already set unless `replace`.
 * `{key: "default"}` removes the key. No printing.
 */
export async function envUpdate(
  ctx: Context,
  app: string,
  name: string,
  opts: {
    planes: Record<string, PlaneValue>;
    /** Warehouse overrides to set or clear, merged into `planes.warehouse`. */
    destinations?: DestinationFlags;
    /** The fetch plane's whole host list; implies `fetch=allow`. */
    fetchHosts?: readonly string[];
    replace?: boolean;
  }
): Promise<Environment> {
  const environment = parseAppEnv(name);
  const flags = validatePlaneMap(opts.planes);
  const fetchHosts = opts.fetchHosts ?? [];
  if (
    Object.keys(flags).length === 0 &&
    noDestinationEdits(opts.destinations) &&
    fetchHosts.length === 0
  ) {
    throw usageError(
      "env update needs at least one --plane, --destination or --fetch-host",
      "e.g. --plane storage=empty, --plane storage=default to clear a key, or --destination clickhouse=clickhouse_dev"
    );
  }
  const creds = staffCreds(ctx);
  refuseOutsideOwnSandbox(creds, environment, "update");
  const { appId } = await resolveApp(creds, app);
  const current = opts.replace ? undefined : await fetchEnvironment(creds, appId, environment);
  const merged = mergePlanes(current?.planes_set, flags, Boolean(opts.replace));
  const withDestinations = opts.destinations
    ? applyDestinations(merged, opts.destinations)
    : merged;
  const planes = applyFetchHosts(
    withDestinations,
    fetchHosts,
    (current?.planes_set as Record<string, unknown> | null | undefined)?.fetch
  );
  const response = await request({
    target: creds.target,
    path: `${environmentsPath(appId, environment)}/planes`,
    method: "PUT",
    body: JSON.stringify({ planes }),
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  return parseJson(response.body) as Environment;
}

/**
 * Reset the named planes to a clean copy of the parent's, as it is now or as of
 * `at` (a sandbox's OLTP branch is the plane that takes one). No printing.
 */
export async function envReset(
  ctx: Context,
  app: string,
  name: string,
  planes: readonly string[],
  at?: string
): Promise<EnvReset> {
  const environment = parseAppEnv(name);
  const keys = parsePlaneKeys(planes);
  const forkAt = at === undefined ? undefined : requireForkAt(at);
  const creds = staffCreds(ctx);
  refuseOutsideOwnSandbox(creds, environment, "reset");
  const { appId } = await resolveApp(creds, app);
  const response = await request({
    target: creds.target,
    path: `${environmentsPath(appId, environment)}/reset`,
    method: "POST",
    body: JSON.stringify(forkAt === undefined ? { planes: keys } : { planes: keys, at: forkAt }),
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  return parseJson(response.body) as EnvReset;
}

export async function runEnvUpdate(
  ctx: Context,
  app: string,
  name: string,
  opts: {
    plane?: string[];
    destination?: string[];
    clearDestination?: string[];
    fetchHost?: string[];
    replace?: boolean;
    json: boolean;
  }
): Promise<void> {
  const planes = parsePlaneFlags(opts.plane ?? []);
  const destinations = parseDestinationFlags(opts.destination ?? [], opts.clearDestination ?? []);
  const fetchHosts = parseFetchHosts(opts.fetchHost ?? []);
  printEnvironment(
    await envUpdate(ctx, app, name, { planes, destinations, fetchHosts, replace: opts.replace }),
    opts.json
  );
}

/** One line per plane, for the human output. */
export function describeReset(reset: EnvReset): string[] {
  return reset.planes.map((p) => {
    const run = p.run_id ? ` (run ${p.run_id})` : "";
    return `${p.plane}: ${p.status}${run}`;
  });
}

export async function runEnvReset(
  ctx: Context,
  app: string,
  name: string,
  opts: { plane?: string[]; at?: string; json: boolean }
): Promise<void> {
  const reset = await envReset(ctx, app, name, opts.plane ?? [], opts.at);
  if (opts.json) {
    process.stdout.write(`${JSON.stringify(reset)}\n`);
    return;
  }
  process.stdout.write(
    `${[`${reset.name} — reset`, ...describeReset(reset).map((l) => `  ${l}`)].join("\n")}\n`
  );
}
