// checks — what the canary's page proves, one async function per check.
//
// The page is what a live app's visitor loads, so these exercise the browser
// half of the platform: a function invoked over the session cookie, the
// workspace SQL route the bookkeeping app reads through, and the SDK's
// `useQuery`. Each check throws an error that starts with its own name, and
// App.tsx renders the failing name as `data-canary-failed`.

import { AIRHOUSE_DATABASE, AIRHOUSE_READS, seriesMismatch } from "../functions/airhouse-reads";

export type CheckName = "echo" | "sql_query_route" | "sdk_query" | "airhouse_query_route";

/** The workspace database the canary's function writes; the page reads it. */
export const DATABASE = "canary_warehouse";
export const SELECT_ONE = "SELECT 1 AS one";

type Row = Record<string, unknown>;

/** `useFunction("echo").invoke` answers with the nonce it was sent. */
export async function checkEcho(invoke: (body: unknown) => Promise<unknown>): Promise<void> {
  await named("echo", async () => {
    const nonce = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
    const result = await invoke({ nonce });
    const echoed =
      isRecord(result) && result.ok === true && isRecord(result.echo)
        ? result.echo.nonce
        : undefined;
    if (echoed !== nonce) throw new Error("the function did not send the nonce back");
  });
}

/**
 * `POST /api/{projectId}/sql/query`, the route the bookkeeping app reads through
 * instead of `useQuery`. Request and response handling are copied from its
 * `useSqlQuery` hook (customer-apps `apps/pokehouse/bookkeeping`), so a change
 * that breaks that hook breaks this check.
 */
export async function checkSqlQueryRoute(): Promise<void> {
  await named("sql_query_route", async () => {
    const { projectId, apiBaseUrl } = runtime();
    const res = await fetch(`${apiBaseUrl}/api/${projectId}/sql/query`, {
      method: "POST",
      credentials: "include",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ sql: SELECT_ONE, database: DATABASE })
    });
    const text = await res.text();
    if (!res.ok) {
      throw new Error(`query failed (${res.status}): ${text.slice(0, 300)}`);
    }
    expectOne(toRows(JSON.parse(text)));
  });
}

/** The SDK's `useQuery` against the same database, settled into a promise by App.tsx. */
export async function checkSdkQuery(settled: Promise<Row[]>): Promise<void> {
  await named("sdk_query", async () => expectOne(await settled));
}

/**
 * Whether this page load runs `airhouse_query_route`: only when its URL says
 * `?airhouse=1`. The check reads `canary_airhouse`, which a deployment has only
 * once its setup added it (README.md, "Per-deployment setup"), so the URL each
 * caller opens is where that is recorded: `.github/custom-app-checks.json` per
 * environment, and the checkpoint's `--airhouse`.
 */
export function airhouseRequested(search: string): boolean {
  return new URLSearchParams(search).get("airhouse") === "1";
}

/**
 * `POST /api/projects/{projectId}/query`, the route the SDK's `useQuery` reads
 * through, against `canary_airhouse`: zero, one and 2,500 rows over Airhouse's
 * Postgres wire (functions/airhouse-reads.ts). The request is the one the SDK's
 * `sharedQuery` sends, plus `?refresh=1`: the route keeps a successful answer
 * for 30 seconds, and a check served from that cache never reaches Airhouse.
 */
export async function checkAirhouseQueryRoute(): Promise<void> {
  await named("airhouse_query_route", async () => {
    const { projectId, apiBaseUrl } = runtime();
    for (const read of AIRHOUSE_READS) {
      const res = await fetch(`${apiBaseUrl}/api/projects/${projectId}/query?refresh=1`, {
        method: "POST",
        credentials: "include",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ sql: read.sql, database: AIRHOUSE_DATABASE })
      });
      const text = await res.text();
      if (!res.ok) {
        throw new Error(`${read.name}: query failed (${res.status}): ${text.slice(0, 300)}`);
      }
      const wrong = seriesMismatch(toRows(JSON.parse(text)), read.rows);
      if (wrong) throw new Error(`${read.name}: ${wrong}`);
    }
  });
}

async function named(check: CheckName, run: () => Promise<void>): Promise<void> {
  try {
    await run();
  } catch (err) {
    throw new Error(`${check}: ${err instanceof Error ? err.message : String(err)}`);
  }
}

function expectOne(rows: Row[]): void {
  if (rows.length !== 1 || Number(rows[0].one) !== 1) {
    throw new Error(`expected one row with one = 1, got ${rows.length} rows`);
  }
}

// ── copied from bookkeeping's useSqlQuery ───────────────────────────────────

interface OxyAppRuntime {
  projectId: string;
  apiBaseUrl?: string;
}

function runtime(): OxyAppRuntime {
  const w = (globalThis as { __OXY_APP__?: OxyAppRuntime }).__OXY_APP__;
  if (!w?.projectId) {
    throw new Error("window.__OXY_APP__.projectId is missing");
  }
  return { projectId: w.projectId, apiBaseUrl: w.apiBaseUrl ?? "" };
}

// `/sql/query` returns positional rows: `[[col0, col1, ...], [v0, v1, ...], ...]`
// where row 0 is the column names. Some builds instead return
// `{ columns: [...], rows: [[...], ...] }`. Normalise both into objects
// keyed by column name.
function toRows(payload: unknown): Row[] {
  let columns: string[];
  let body: unknown[][];
  if (Array.isArray(payload)) {
    const [head, ...rest] = payload as unknown[][];
    columns = (head ?? []).map(String);
    body = rest;
  } else if (payload && typeof payload === "object" && "rows" in payload) {
    const p = payload as { columns?: unknown[]; rows?: unknown[][] };
    columns = (p.columns ?? []).map(String);
    body = p.rows ?? [];
  } else {
    return [];
  }
  return body.map((row) => Object.fromEntries(columns.map((c, i) => [c, row[i]])));
}

function isRecord(value: unknown): value is Row {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
