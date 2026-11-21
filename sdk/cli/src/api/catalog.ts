/**
 * What endpoints exist, and what each one expects.
 *
 * Discovery is the reason this tool is worth building for an agent: without it
 * the caller needs a checkout to know the surface, and the whole premise is a
 * caller who has neither a checkout nor a doc site open.
 *
 * The Rust `oxy api` answered this from data baked into the binary at build
 * time — a build script walked the axum router source and
 * emitted a table. That has one flaw this design fixes: the table describes the
 * routes the BINARY could mount, and several mounts are mode-conditional, so a
 * listed path can still 404 on the deployment in front of you. Asking the
 * deployment is strictly more truthful.
 *
 * It costs an authenticated network call, so the answer is cached per host and
 * a failure degrades to STALE AND SAID SO rather than to nothing — the
 * discipline `customer-tooling`'s `lib/customers.sh` writes down at length,
 * and for the same reason: an empty answer here reads as "this deployment has
 * no such route", which is a lie that sends the caller off to debug the wrong
 * thing.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { hostKey } from "../auth/credentials.js";
import * as log from "../ui/log.js";
import { CliError, ExitCode } from "../util/errors.js";
import { ensureDir, oxycCacheDir, slugifyForPath } from "../util/paths.js";
import { parseJson, request } from "./request.js";

/** One endpoint, as `/api/_catalog` reports it. Mirrors Rust `RouteDescription`. */
export interface CatalogRoute {
  method: string;
  path: string;
  surface: string;
  /** The credential this surface expects, spelled out rather than implied. */
  credential: string;
  path_parameters: string[];
  description: string;
  note: string;
  handler: string;
  /** `ide-only`, `worker-only` or `fleet-ok` — the role_manifest's own answer. */
  role: string;
}

export interface Catalog {
  routes: CatalogRoute[];
  surfaces: Array<{ id: string; label: string; credential: string }>;
  /** Which deployment answered, and when — both shown when serving stale. */
  host: string;
  fetchedAt: number;
  /** True when this came off disk after the network failed. */
  stale?: boolean;
}

/** An hour: long enough that a session costs one call, short enough to notice a deploy. */
const CATALOG_TTL_MS = 3_600_000;

function catalogPath(target: string): string {
  // 0700/0600 like the response cache: a full route table for a deployment
  // is reconnaissance, which is why the endpoint serving it is authenticated.
  const dir = ensureDir(join(oxycCacheDir(), "catalog"), 0o700);
  return join(dir, `${slugifyForPath(hostKey(target))}.json`);
}

function readDisk(target: string): Catalog | undefined {
  try {
    const cached = JSON.parse(readFileSync(catalogPath(target), "utf8")) as Catalog;
    // The filename is a sanitised host and can in principle collide; the
    // stored host is the identity. Serving one deployment's route table as
    // another's would be indistinguishable from a routing bug.
    if (cached.host !== hostKey(target)) return undefined;
    return cached;
  } catch {
    return undefined;
  }
}

function writeDisk(target: string, catalog: Catalog): void {
  try {
    writeFileSync(catalogPath(target), JSON.stringify(catalog), { mode: 0o600 });
  } catch {
    // A cache that cannot be written costs a call per command, not an answer.
  }
}

export interface CatalogOptions {
  target: string;
  bearer?: string;
  /** Ignore a fresh cache and ask the deployment again. */
  refresh?: boolean;
}

/**
 * The route table for `target`.
 *
 * Fresh cache → live `/api/_catalog` → stale cache with a warning → an error
 * that says which of the two things went wrong. Never an empty catalog: an
 * empty list would be reported as "no such route" by every caller.
 *
 * ASKS FOR THE WHOLE TABLE even though the endpoint takes a `?filter=`,
 * because the cache is the point: one full fetch per host per hour serves
 * every later `oxyc routes <anything>` offline, where a filtered fetch would
 * be a round trip per query and could not be cached under one key without
 * poisoning it — a cached "routes matching threads" is not a route table, and
 * serving it as one would report every other endpoint as missing. `?filter=`
 * still earns its place on the endpoint, for the cache-less caller: a script,
 * a `curl`, a one-off.
 */
export async function loadCatalog(opts: CatalogOptions): Promise<Catalog> {
  const cached = readDisk(opts.target);
  if (!opts.refresh && cached && Date.now() - cached.fetchedAt < CATALOG_TTL_MS) {
    return cached;
  }

  // NO CREDENTIAL AND NOTHING CACHED: say so before asking, because the answer
  // an unauthenticated request gets back is not diagnostic. `/api/_catalog` is
  // on the protected router, and an anonymous request to it comes back 404 —
  // which this code would otherwise report as "that deployment predates route
  // discovery", sending the reader off to check a deployment version when what
  // they actually need is to log in. With a cache in hand the stale path below
  // is still better than an error, so the check is only for the empty case.
  if (!opts.bearer && !cached) {
    throw new CliError(`not authenticated for ${opts.target}`, {
      code: ExitCode.AUTH,
      hint: `oxyc login --target ${opts.target}   — route discovery needs a token`
    });
  }

  try {
    const response = await request({
      target: opts.target,
      path: "/api/_catalog",
      method: "GET",
      bearer: opts.bearer,
      timeoutMs: 30_000
    });

    if (response.status === 401 || response.status === 403) {
      if (cached) return serveStale(cached, "not authenticated for the route catalog");
      throw new CliError(`the route catalog at ${opts.target} rejected this token`, {
        code: ExitCode.AUTH,
        hint: `oxyc login --target ${opts.target}`
      });
    }
    if (response.status === 404) {
      // An older deployment that predates `/api/_catalog`. Not an error the
      // caller can act on beyond "this one cannot tell you", so say exactly
      // that rather than "not found", which reads as a bad path.
      if (cached) return serveStale(cached, `${opts.target} has no /api/_catalog (older build)`);
      throw new CliError(`${opts.target} does not serve /api/_catalog`, {
        code: ExitCode.NOT_FOUND,
        hint: "that deployment predates route discovery — try `oxyc openapi` for the documented subset"
      });
    }
    if (response.status < 200 || response.status >= 300) {
      if (cached) return serveStale(cached, `catalog request failed (${response.status})`);
      throw new CliError(`could not read the route catalog (${response.status})`, {
        code: ExitCode.UNAVAILABLE
      });
    }

    const payload = parseJson(response.body) as Partial<Catalog> | undefined;
    if (!payload?.routes?.length) {
      // An EMPTY catalog is refused, never served. It is indistinguishable
      // from a deployment with no routes, and every caller downstream would
      // report a real endpoint as nonexistent.
      if (cached) return serveStale(cached, "the deployment returned an empty route catalog");
      throw new CliError("the deployment returned an empty route catalog", {
        code: ExitCode.UNAVAILABLE,
        hint: "this is a server bug, not a bad path — report it rather than working around it"
      });
    }

    const catalog: Catalog = {
      routes: payload.routes,
      surfaces: payload.surfaces ?? [],
      host: hostKey(opts.target),
      fetchedAt: Date.now()
    };
    writeDisk(opts.target, catalog);
    return catalog;
  } catch (cause) {
    if (cause instanceof CliError && !cached) throw cause;
    if (cached) return serveStale(cached, (cause as Error).message);
    throw cause;
  }
}

/**
 * Serve the cache and say so.
 *
 * The warning is not decoration. A stale route table is usually right and
 * occasionally wrong in the one way that matters — a route added since the
 * cache was written reads as nonexistent — so the reader has to know which
 * kind of answer they are holding.
 */
function serveStale(cached: Catalog, why: string): Catalog {
  const ageMinutes = Math.round((Date.now() - cached.fetchedAt) / 60_000);
  log.warn(`serving a STALE route catalog (${ageMinutes}m old): ${why}`);
  log.hint("oxyc routes --refresh   once the deployment is reachable");
  return { ...cached, stale: true };
}

/** Routes whose method, path or surface contains `needle`, case-insensitively. */
export function searchRoutes(catalog: Catalog, needle: string | undefined): CatalogRoute[] {
  if (!needle) return catalog.routes;
  const n = needle.toLowerCase();
  return catalog.routes.filter(
    (r) =>
      r.path.toLowerCase().includes(n) ||
      r.method.toLowerCase().includes(n) ||
      r.surface.toLowerCase().includes(n) ||
      r.description.toLowerCase().includes(n)
  );
}

/** The OpenAPI document the deployment publishes. */
export async function loadOpenApi(opts: CatalogOptions): Promise<unknown> {
  const response = await request({
    target: opts.target,
    path: "/apidoc/openapi.json",
    method: "GET",
    bearer: opts.bearer,
    timeoutMs: 30_000
  });
  if (response.status < 200 || response.status >= 300) {
    throw new CliError(`could not read the OpenAPI document (${response.status})`, {
      code: ExitCode.UNAVAILABLE,
      hint: `check ${opts.target}/apidoc is served by this deployment`
    });
  }
  const doc = parseJson(response.body);
  if (doc === undefined) {
    throw new CliError("the OpenAPI endpoint did not return JSON", {
      code: ExitCode.UNAVAILABLE
    });
  }
  return doc;
}
