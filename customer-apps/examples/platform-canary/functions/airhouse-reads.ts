// airhouse-reads — the reads both Airhouse checks make, and what they must return.
//
// The function's `airhouse_reads` step sends them through `ctx.warehouse.query`
// (steps.ts); the page's `airhouse_query_route` check sends them to
// `/api/projects/{id}/query`, the route the SDK's `useQuery` reads through
// (src/checks.ts). One list, so the two cannot drift. No imports: the page's
// Vite build and the function's esbuild bundle both take this file.

/**
 * The workspace's `airhouse_managed` database, read only: its reads go over
 * Airhouse's Postgres wire, which `canary_warehouse` (ClickHouse on staging)
 * does not touch. README.md, "Per-deployment setup", adds it.
 */
export const AIRHOUSE_DATABASE = "canary_airhouse";

/**
 * Each read returns `n` = 1…`rows`, exactly once. Zero rows is the case
 * 2026-10-28's P0 broke for two days (#3791, fixed in #3871): every empty
 * Airhouse result failed — 500 `failed to convert query results` on `/query`,
 * `row stream: driver error: connection closed` in functions. 2,500 rows spans
 * three of the 1,000-row pages #3791 replaced, which duplicated and dropped rows.
 */
export const AIRHOUSE_READS: readonly { name: string; sql: string; rows: number }[] = [
  { name: "zero rows", sql: "SELECT 1 AS n WHERE 1 = 0", rows: 0 },
  { name: "one row", sql: "SELECT 1 AS n", rows: 1 },
  { name: "2500 rows", sql: "SELECT n FROM generate_series(1, 2500) AS t(n)", rows: 2500 }
];

/** Why `rows` is not `n` = 1…`count` once each (any order), or `null` when it is. */
export function seriesMismatch(rows: Record<string, unknown>[], count: number): string | null {
  const seen = new Set(rows.map((row) => Number(row.n)));
  const exact =
    rows.length === count && seen.size === count && [...seen].every((n) => n >= 1 && n <= count);
  if (exact) return null;
  const want = count === 0 ? "no rows" : `${count} rows, n = 1 to ${count} once each`;
  return `expected ${want}; got ${rows.length} rows, ${seen.size} distinct`;
}
