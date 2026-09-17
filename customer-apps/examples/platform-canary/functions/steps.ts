// steps — everything the platform canary checks, as functions over `ctx`.
//
// Each step exercises one host API that live custom apps depend on (README.md
// has the table). A failing step is re-thrown as
// `canary step <name> failed: <cause>`. The function-failure pager fingerprints
// the first 240 characters of that message, so a step name near the start gives
// each step its own fingerprint, and its own page.
//
// Nothing here reads the request or the environment: `canary.ts` does, so
// `steps.test.ts` can drive every step with a fake `ctx`.

// Types only. `@oxy-hq/sdk`'s root entry also loads React, whose CommonJS entry
// reads `process.env` at load; the isolate has no `process`, so a value import
// would fail every run before a step starts (steps.test.ts bundles and checks).
import type { OxyFunctionContext, OxyFunctionRow, StorageUploadUrl } from "@oxy-hq/sdk";
import { runShapeZoo } from "./shape-zoo";

export type StepName =
  | "warehouse_insert"
  | "warehouse_exec"
  | "warehouse_readback"
  | "sql_read"
  | "oltp_roundtrip"
  | "shape_zoo"
  | "org_read"
  | "storage_roundtrip"
  | "secrets_roundtrip"
  | "check_in";

/** Run order. `check_in` is last: it reports a run in which every other step passed. */
export const ALL_STEPS: StepName[] = [
  "warehouse_insert",
  "warehouse_exec",
  "warehouse_readback",
  "sql_read",
  "oltp_roundtrip",
  "shape_zoo",
  "org_read",
  "storage_roundtrip",
  "secrets_roundtrip",
  "check_in"
];

/** The workspace database the canary writes: `destinations` in oxy-app.json. */
export const DATABASE = "canary_warehouse";
const TABLE = "oxy_canary_writes";
const OLTP_TABLE = "canary_roundtrip";
const OCTET = "application/octet-stream";

/** The app secret each run writes, and the next run reads back through `ctx.env`. */
export const CANARY_SECRET_KEY = "CANARY_SECRET_ROUNDTRIP";

/**
 * How old the previous run's secret may be. The schedule fires every five
 * minutes, so an hour is twelve runs: a late or skipped tick never trips it,
 * and writes that stopped reaching `ctx.env` still page the same hour.
 */
const SECRET_MAX_AGE_MS = 60 * 60 * 1000;

/** Run ids are inlined into warehouse SQL, so they are held to this alphabet. */
const RUN_ID_RE = /^[a-z0-9-]{1,64}$/;

const INSERT_PATHS = ["insert-1", "insert-2", "insert-3"];
const EXEC_PATHS = ["exec-1", "exec-2"];

export interface CanaryOptions {
  runId: string;
  steps: StepName[];
  checkinUrl?: string;
}

interface RunState extends CanaryOptions {
  ctx: OxyFunctionContext;
  tableReady: boolean;
}

type FetchInit = NonNullable<Parameters<OxyFunctionContext["fetch"]>[1]>;

/** Parse `CANARY_STEPS`. Absent or blank means every step; an unknown name throws. */
export function selectSteps(csv: string | undefined): StepName[] {
  if (csv === undefined || csv.trim() === "") return [...ALL_STEPS];
  const named = new Set(
    csv
      .split(",")
      .map((name) => name.trim())
      .filter(Boolean)
  );
  if (named.size === 0) throw new Error("CANARY_STEPS names no step");
  for (const name of named) {
    if (!(ALL_STEPS as string[]).includes(name)) {
      throw new Error(
        `CANARY_STEPS names an unknown step "${name}"; known: ${ALL_STEPS.join(", ")}`
      );
    }
  }
  return ALL_STEPS.filter((step) => named.has(step));
}

/** Run the given steps in `ALL_STEPS` order, stopping at the first failure. */
export async function runCanary(
  ctx: OxyFunctionContext,
  opts: CanaryOptions
): Promise<{ ok: true; runId: string; steps: StepName[] }> {
  if (!RUN_ID_RE.test(opts.runId)) {
    throw new Error("canary run id must be 1-64 lowercase letters, digits or hyphens");
  }
  const steps = ALL_STEPS.filter((step) => opts.steps.includes(step));
  const run: RunState = { ...opts, steps, ctx, tableReady: false };
  for (const name of steps) {
    try {
      await STEPS[name](run);
    } catch (err) {
      throw new Error(`canary step ${name} failed: ${causeOf(err)}`, { cause: err });
    }
  }
  return { ok: true, runId: opts.runId, steps };
}

const STEPS: Record<StepName, (run: RunState) => Promise<void>> = {
  warehouse_insert: warehouseInsert,
  warehouse_exec: warehouseExec,
  warehouse_readback: warehouseReadback,
  sql_read: sqlRead,
  oltp_roundtrip: oltpRoundtrip,
  // Every zoo case: ClickHouse through ctx.warehouse on DATABASE, Postgres through ctx.oltp.
  shape_zoo: (run) => runShapeZoo(run.ctx, DATABASE),
  org_read: orgRead,
  storage_roundtrip: storageRoundtrip,
  secrets_roundtrip: secretsRoundtrip,
  check_in: checkIn
};

// ── warehouse ────────────────────────────────────────────────────────────────

/** Created on first use. The 7-day TTL is the cleanup: the canary never deletes these rows. */
async function ensureTable(run: RunState): Promise<void> {
  if (run.tableReady) return;
  await run.ctx.warehouse.exec(
    DATABASE,
    `CREATE TABLE IF NOT EXISTS ${TABLE} (
       run String,
       path LowCardinality(String),
       written_at DateTime DEFAULT now()
     ) ENGINE = MergeTree
     ORDER BY (written_at, run)
     TTL written_at + INTERVAL 7 DAY`
  );
  run.tableReady = true;
}

/** `ctx.warehouse.insert` with three rows: the quoted `INSERT … VALUES` list app uploads send. */
async function warehouseInsert(run: RunState): Promise<void> {
  await ensureTable(run);
  const rows = INSERT_PATHS.map((path) => ({ run: run.runId, path }));
  await run.ctx.warehouse.insert(DATABASE, TABLE, rows);
}

/** `ctx.warehouse.exec` of a two-row INSERT whose first token is a comment, not INSERT. */
async function warehouseExec(run: RunState): Promise<void> {
  await ensureTable(run);
  const values = EXEC_PATHS.map((path) => `('${run.runId}', '${path}')`).join(", ");
  await run.ctx.warehouse.exec(
    DATABASE,
    `-- platform-canary: a multi-row statement that opens with a comment\nINSERT INTO ${TABLE} (run, path) VALUES ${values}`
  );
}

/** Every row the write steps sent is there. A write that returned without landing fails. */
async function warehouseReadback(run: RunState): Promise<void> {
  const expected = [
    ...(run.steps.includes("warehouse_insert") ? INSERT_PATHS : []),
    ...(run.steps.includes("warehouse_exec") ? EXEC_PATHS : [])
  ].sort();
  if (expected.length === 0) {
    throw new Error("nothing to read back: run warehouse_insert or warehouse_exec with it");
  }
  const { rows } = await run.ctx.warehouse.query(
    DATABASE,
    `SELECT path FROM ${TABLE} WHERE run = '${run.runId}' ORDER BY path`
  );
  const landed = rows.map((row) => String(row.path)).sort();
  if (landed.join(",") !== expected.join(",")) {
    throw new Error(`${landed.length} of ${expected.length} rows landed`);
  }
}

// ── reads ────────────────────────────────────────────────────────────────────

/**
 * Top-level `ctx.query`. The host resolves `{ rows, truncated }` (`host.rs`
 * `query`), which is what live functions destructure. The SDK's type says a bare
 * row array, so the result is checked as `unknown`: a host that changed shape
 * would break every `const { rows } = await ctx.query(…)`, and must fail here.
 */
async function sqlRead(run: RunState): Promise<void> {
  const result: unknown = await run.ctx.query("SELECT 1 AS one");
  if (!isRecord(result) || !Array.isArray(result.rows) || typeof result.truncated !== "boolean") {
    throw new Error("ctx.query did not resolve { rows, truncated }");
  }
  const rows = result.rows as OxyFunctionRow[];
  if (rows.length !== 1 || Number(rows[0].one) !== 1) {
    throw new Error(`expected one row with one = 1, got ${rows.length} rows`);
  }
}

/** `ctx.org.places()` answers the shape Store Ops reads. An org with no places passes. */
async function orgRead(run: RunState): Promise<void> {
  const result: unknown = await run.ctx.org.places();
  if (!isRecord(result) || !Array.isArray(result.places) || typeof result.total !== "number") {
    throw new Error("ctx.org.places() did not resolve { places: [...], total }");
  }
  const bad = (result.places as unknown[]).findIndex((place) => !isPlace(place));
  if (bad >= 0) {
    throw new Error(`place ${bad} lacks id, name, parent_id, status, timezone or external_ids`);
  }
}

function isPlace(value: unknown): boolean {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.name === "string" &&
    (value.parent_id === null || typeof value.parent_id === "string") &&
    typeof value.status === "string" &&
    typeof value.timezone === "string" &&
    isRecord(value.external_ids)
  );
}

// ── oltp ─────────────────────────────────────────────────────────────────────

/** Write, read and delete one row in the app's own OLTP schema. A failed delete fails the step. */
async function oltpRoundtrip(run: RunState): Promise<void> {
  const { oltp } = run.ctx;
  await oltp.exec(
    `CREATE TABLE IF NOT EXISTS ${OLTP_TABLE} (
       run        text        PRIMARY KEY,
       written_at timestamptz NOT NULL DEFAULT now()
     )`
  );
  await oltp.exec(`INSERT INTO ${OLTP_TABLE} (run) VALUES ($1)`, [run.runId]);
  await withCleanup(
    async () => {
      const rows = await oltp.query(`SELECT run FROM ${OLTP_TABLE} WHERE run = $1`, [run.runId]);
      if (rows.length !== 1 || rows[0].run !== run.runId) {
        throw new Error(`read back ${rows.length} rows for this run, expected 1`);
      }
    },
    async () => {
      const deleted = await oltp.exec(`DELETE FROM ${OLTP_TABLE} WHERE run = $1`, [run.runId]);
      if (deleted !== 1) throw new Error(`cleanup deleted ${deleted} rows, expected 1`);
    }
  );
}

// ── storage ──────────────────────────────────────────────────────────────────

/** Put and get binary as base64, upload through a presigned PUT, head it, delete both. */
async function storageRoundtrip(run: RunState): Promise<void> {
  const written: string[] = [];
  await withCleanup(
    () => storageWrites(run, written),
    async () => {
      if (written.length > 0) await run.ctx.storage.delete(written);
    }
  );
}

async function storageWrites(run: RunState, written: string[]): Promise<void> {
  const { storage } = run.ctx;
  // Every byte value: a UTF-8 decode anywhere on the path mangles the upper half.
  const bytes = Uint8Array.from({ length: 256 }, (_, i) => i);
  const body = bytesToBase64(bytes);

  const put = await storage.put(`canary/${run.runId}.bin`, body, {
    encoding: "base64",
    contentType: OCTET
  });
  written.push(put.key);
  const got = await storage.get(put.key, { encoding: "base64" });
  if (!got) throw new Error("get found nothing at the key put just wrote");
  if (!sameBytes(base64ToBytes(got.body), bytes)) {
    throw new Error("get returned different bytes than put wrote");
  }

  const upload = await storage.getUploadUrl({
    pathname: `canary/${run.runId}-upload.bin`,
    contentType: OCTET,
    contentLength: bytes.length
  });
  written.push(upload.key);
  const res = await run.ctx.fetch(upload.url, uploadInit(upload, body));
  if (res.status < 200 || res.status >= 300) {
    throw new Error(`presigned PUT answered HTTP ${res.status}`);
  }
  const head = await storage.head(upload.key);
  if (!head || head.size !== bytes.length) {
    throw new Error(
      `head saw ${head ? head.size : "no"} bytes after the PUT, expected ${bytes.length}`
    );
  }
}

function uploadInit(upload: StorageUploadUrl, body: string): FetchInit {
  const headers: Record<string, string> = { "content-type": OCTET };
  // Signed into the URL when the app declares a retention rule. This app declares none.
  if (upload.tagging) headers["x-amz-tagging"] = upload.tagging;
  // The host honours `bodyEncoding` (`host.rs` `fetch_body_bytes`); the SDK's
  // `OxyFetchInit` does not declare it yet.
  const init: FetchInit & { bodyEncoding: "base64" } = {
    method: "PUT",
    headers,
    body,
    bodyEncoding: "base64"
  };
  return init;
}

// Base64 through the isolate's `btoa`/`atob` (oxy-globals.d.ts), which take a
// Latin1 string: one character per byte, so every byte value survives.
function bytesToBase64(bytes: Uint8Array): string {
  let latin1 = "";
  for (const byte of bytes) latin1 += String.fromCharCode(byte);
  return btoa(latin1);
}

function base64ToBytes(base64: string): Uint8Array {
  return Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((byte, i) => byte === b[i]);
}

// ── secrets ──────────────────────────────────────────────────────────────────

/**
 * `ctx.env` is resolved when a run starts, so a value set now is only readable
 * by the next run. Each run writes `<runId>@<ms>` and checks the value the
 * previous run wrote. The very first run has nothing to read and fails once.
 */
async function secretsRoundtrip(run: RunState): Promise<void> {
  const previous = run.ctx.env[CANARY_SECRET_KEY];
  const now = Date.now();
  // Write first, so the next run has a value to read even when this check fails.
  await run.ctx.secrets.set(CANARY_SECRET_KEY, `${run.runId}@${now}`);
  if (!previous) {
    throw new Error(`ctx.env has no ${CANARY_SECRET_KEY}: no earlier ctx.secrets.set reached it`);
  }
  const match = /^[a-z0-9-]+@(\d+)$/.exec(previous);
  if (!match) throw new Error(`ctx.env ${CANARY_SECRET_KEY} is not a value this canary writes`);
  const minutes = Math.round((now - Number(match[1])) / 60_000);
  if (now - Number(match[1]) > SECRET_MAX_AGE_MS) {
    throw new Error(
      `ctx.env ${CANARY_SECRET_KEY} is ${minutes} minutes old: writes are not reaching ctx.env, or the canary stopped running`
    );
  }
}

// ── check-in ─────────────────────────────────────────────────────────────────

/** POST the All Quiet check-in. Runs last, so it only reports a run that passed. */
async function checkIn(run: RunState): Promise<void> {
  if (!run.checkinUrl) throw new Error("CANARY_CHECKIN_URL is not set");
  const res = await run.ctx.fetch(run.checkinUrl, { method: "POST" });
  if (res.status < 200 || res.status >= 300) {
    throw new Error(`check-in answered HTTP ${res.status}`);
  }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/**
 * Run `body`, then `cleanup` whether or not it failed. A failed cleanup fails
 * the step; when both fail, the body's error leads and the cleanup's follows.
 */
async function withCleanup(body: () => Promise<void>, cleanup: () => Promise<void>): Promise<void> {
  try {
    await body();
  } catch (err) {
    try {
      await cleanup();
    } catch (cleanupErr) {
      throw new Error(`${causeOf(err)}; cleanup also failed: ${causeOf(cleanupErr)}`, {
        cause: err
      });
    }
    throw err;
  }
  await cleanup();
}

/** The message, with URLs removed: the check-in URL and a presigned PUT are both credentials. */
function causeOf(err: unknown): string {
  const message = err instanceof Error ? err.message : String(err);
  return message.replace(/https?:\/\/[^\s)'"]+/g, "<url>");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
