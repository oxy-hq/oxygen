// Shared in-flight dedup + short result cache for useQuery. Module-level so
// every useQuery across the tree shares one cache. A shared in-flight request
// is intentionally NOT aborted on a single consumer's unmount — others may
// still need it; consumers guard their own setState with a `cancelled` flag.

import { apiErrorFromResponse } from "./errors";
import type { SentQueryParam } from "./query-params";

export type QueryResult = { columns: string[]; rows: unknown[][] };
export type Fetcher = (path: string, init: RequestInit) => Promise<Response>;

const SWR_TTL_MS = 30_000;
const inflight = new Map<string, Promise<QueryResult>>();
const cache = new Map<string, { at: number; data: QueryResult }>();

/** The params a SQL template names, as `paramsToSend` builds them. */
export type QueryParams = Record<string, SentQueryParam>;

/**
 * `params` is part of the key: the server substitutes them into `sql`, so the
 * same template with different params is a different query. NUL separates
 * them from the SQL, which cannot hold one, so no template can spell another
 * query's key.
 */
export function queryKey(
  projectId: string,
  db: string | undefined,
  sql: string,
  params?: QueryParams
): string {
  const base = `${projectId} ${db ?? ""} ${sql}`;
  return params ? `${base}\u0000${JSON.stringify(params)}` : base;
}

export function getCached(
  projectId: string,
  sql: string,
  db: string | undefined,
  params?: QueryParams
): QueryResult | undefined {
  const e = cache.get(queryKey(projectId, db, sql, params));
  return e && Date.now() - e.at < SWR_TTL_MS ? e.data : undefined;
}

/** Fetch with in-flight dedup + cache. `force` bypasses the fresh-cache
 *  short-circuit (used by refetch) but still dedupes a concurrent in-flight.
 *
 *  `params` are sent beside `sql`, not written into it: the server
 *  substitutes each `{{ params.X }}` placeholder and quotes a string by the
 *  rule of the engine that will read it (see ./query-params). */
export async function sharedQuery(
  fetcher: Fetcher,
  projectId: string,
  sql: string,
  db: string | undefined,
  opts: { force?: boolean; params?: QueryParams } = {}
): Promise<QueryResult> {
  const key = queryKey(projectId, db, sql, opts.params);
  if (!opts.force) {
    const fresh = getCached(projectId, sql, db, opts.params);
    if (fresh) return fresh;
  }
  const existing = inflight.get(key);
  if (existing) return existing;

  const body = JSON.stringify({
    sql,
    ...(db ? { database: db } : {}),
    ...(opts.params ? { params: opts.params } : {})
  });
  const p = (async () => {
    const resp = await fetcher(`/api/projects/${projectId}/query`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body
    });
    if (!resp.ok) {
      throw await apiErrorFromResponse(resp);
    }
    const data = (await resp.json()) as QueryResult;
    cache.set(key, { at: Date.now(), data });
    return data;
  })().finally(() => inflight.delete(key));

  inflight.set(key, p);
  return p;
}

/** Test-only: reset module state between tests. */
export function __clearQueryCache(): void {
  inflight.clear();
  cache.clear();
}
