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
  | "upsert_refusal"
  | "tx_refusal"
  | "sql_read"
  | "sql_stream"
  | "oltp_roundtrip"
  | "oltp_transaction"
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
  "upsert_refusal",
  "tx_refusal",
  "sql_read",
  "sql_stream",
  "oltp_roundtrip",
  "oltp_transaction",
  "shape_zoo",
  "org_read",
  "storage_roundtrip",
  "secrets_roundtrip",
  "check_in"
];

/**
 * Every name the host pages an op under: `HOST_OPS` in
 * `crates/app/src/server/api/custom_apps_functions/host_call_attrs.rs`.
 * `crates/app/tests/custom_apps/canary_coverage.rs` holds this union equal to
 * that list, so a new host op fails there until it is named here and either
 * exercised by a step or exempted with a reason.
 */
export type HostOp =
  | "query"
  | "query_stream"
  | "fetch"
  | "semantic.query"
  | "airway.run"
  | "warehouse.insert"
  | "warehouse.exec"
  | "warehouse.upsert"
  | "warehouse.query"
  | "tx.begin"
  | "tx.begin_oltp"
  | "tx.query"
  | "tx.exec"
  | "tx.commit"
  | "tx.rollback"
  | "oltp.query"
  | "oltp.exec"
  | "airhouse.query"
  | "airhouse.exec"
  | "airhouse.append"
  | "storage.getUploadUrl"
  | "storage.getDownloadUrl"
  | "storage.put"
  | "storage.get"
  | "storage.head"
  | "storage.list"
  | "storage.delete"
  | "storage.copy"
  | "secrets.set"
  | "email.send"
  | "org.people"
  | "org.places"
  | "org.assignments";

/**
 * The host ops each step makes, in the order it makes them. Checked both
 * ways: `steps.test.ts` runs each step on a fake host and compares the calls
 * it recorded with this list, and `canary_coverage.rs` checks every host op
 * is in one of these lists or exempted there with a reason. A step's ops are
 * the calls it makes itself, helpers included — `ensureTable`'s
 * `warehouse.exec` for the write steps, the zoo's reads for `shape_zoo`.
 */
export const STEP_OPS: Record<StepName, readonly HostOp[]> = {
  warehouse_insert: ["warehouse.exec", "warehouse.insert"],
  warehouse_exec: ["warehouse.exec"],
  warehouse_readback: ["warehouse.query"],
  upsert_refusal: ["warehouse.upsert"],
  tx_refusal: ["tx.begin"],
  sql_read: ["query"],
  sql_stream: ["query_stream"],
  oltp_roundtrip: ["oltp.exec", "oltp.query"],
  oltp_transaction: [
    "oltp.exec",
    "tx.begin_oltp",
    "tx.exec",
    "tx.query",
    "tx.commit",
    "oltp.query",
    "tx.rollback"
  ],
  shape_zoo: ["warehouse.exec", "warehouse.query", "oltp.exec", "oltp.query"],
  org_read: ["org.places", "org.people", "org.assignments"],
  storage_roundtrip: [
    "storage.put",
    "storage.get",
    "storage.getUploadUrl",
    "fetch",
    "storage.head",
    "storage.copy",
    "storage.getDownloadUrl",
    "storage.list",
    "storage.delete"
  ],
  secrets_roundtrip: ["secrets.set"],
  check_in: ["fetch"]
};

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

/**
 * How `storage.list` walks under `canary/`: pages of `LIST_PAGE_SIZE`, at most
 * `MAX_LIST_PAGES` of them before giving up. Every run deletes what it wrote,
 * so a silo with more objects than that is itself the finding: earlier runs
 * are not cleaning up.
 */
const LIST_PAGE_SIZE = 100;
const MAX_LIST_PAGES = 10;

/**
 * The stable part of each refusal the canary pins on its ClickHouse
 * destination. `upsert_support.rs` and the connector's
 * `transaction::unsupported` write the rest; a change to these words is a
 * change to what live apps and the docs see.
 */
const UPSERT_REFUSAL = /warehouse\.upsert is not supported on ClickHouse/;
const TX_REFUSAL = /ClickHouse does not support multi-statement transactions/;

export interface CanaryOptions {
  runId: string;
  steps: StepName[];
  checkinUrl?: string;
}

interface RunState extends CanaryOptions {
  ctx: OxyFunctionContext;
  tableReady: boolean;
  oltpTableReady: boolean;
}

type FetchInit = NonNullable<Parameters<OxyFunctionContext["fetch"]>[1]>;

/**
 * `ctx.oltp.tx`, as the host serves it (`begin_oltp` in `host.rs`, the same
 * handle as `ctx.tx`). The published `@oxy-hq/sdk` this app installs does not
 * declare it on `OxyOltpApi` yet; the workspace SDK does. Typed here the way
 * `bodyEncoding` is below, until the next SDK release.
 */
interface OltpTransaction {
  query(sql: string, params?: unknown[]): Promise<OxyFunctionRow[]>;
  exec(sql: string, params?: unknown[]): Promise<number>;
}
type OltpWithTx = OxyFunctionContext["oltp"] & {
  tx<T>(fn: (tx: OltpTransaction) => Promise<T> | T): Promise<T>;
};

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
  const run: RunState = { ...opts, steps, ctx, tableReady: false, oltpTableReady: false };
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
  upsert_refusal: upsertRefusal,
  tx_refusal: txRefusal,
  sql_read: sqlRead,
  sql_stream: sqlStream,
  oltp_roundtrip: oltpRoundtrip,
  oltp_transaction: oltpTransaction,
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

// ── refusals the destination's engine decides ────────────────────────────────

/**
 * Pins a refusal, not a write. `ctx.warehouse.upsert` compiles to `ON CONFLICT`,
 * which ClickHouse cannot parse, so the host refuses it by name before sending
 * anything (`upsert_support.rs`); a host that let the statement through would
 * fail in ClickHouse's words, one row late, and the docs and live apps depend
 * on this wording. Nothing is written. The refusal is the app's own condition
 * (`bad_request` in `host_call_attrs.rs`), so catching it here pages nobody.
 */
async function upsertRefusal(run: RunState): Promise<void> {
  await expectRefusal(
    "ctx.warehouse.upsert",
    () =>
      run.ctx.warehouse.upsert(DATABASE, TABLE, [{ run: run.runId, path: "upsert-1" }], ["run"]),
    UPSERT_REFUSAL
  );
}

/**
 * Pins a refusal, not a transaction. `ctx.tx` needs a Postgres-backed
 * database; on ClickHouse the connector refuses to open one, naming the engine
 * (`transaction::unsupported` in the connector crate), and the callback never
 * runs — the alternative, running the statements one by one, would report
 * success on a half-applied write. Same classification as `upsert_refusal`.
 */
async function txRefusal(run: RunState): Promise<void> {
  let ran = false;
  await expectRefusal(
    "ctx.tx",
    () =>
      run.ctx.tx(DATABASE, async () => {
        ran = true;
      }),
    TX_REFUSAL
  );
  if (ran) throw new Error("ctx.tx ran the callback on ClickHouse instead of refusing");
}

/** `call` must reject in words matching `refusal`; resolving, or rejecting in other words, fails. */
async function expectRefusal(
  what: string,
  call: () => Promise<unknown>,
  refusal: RegExp
): Promise<void> {
  let outcome: { refused: false } | { refused: true; message: string };
  try {
    await call();
    outcome = { refused: false };
  } catch (err) {
    outcome = { refused: true, message: causeOf(err) };
  }
  if (!outcome.refused) throw new Error(`${what} resolved on ClickHouse instead of refusing`);
  if (!refusal.test(outcome.message)) {
    throw new Error(`${what} was refused in other words than ${refusal}: ${outcome.message}`);
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

/**
 * `ctx.queryStream`: the same read through the async generator, which the
 * isolate feeds from one `query_stream` host call and yields in batches. A
 * generator that yields nothing, or something other than row arrays, fails.
 */
async function sqlStream(run: RunState): Promise<void> {
  const rows: OxyFunctionRow[] = [];
  for await (const batch of run.ctx.queryStream("SELECT 1 AS one")) {
    if (!Array.isArray(batch)) throw new Error("ctx.queryStream yielded something other than a row array");
    rows.push(...batch);
  }
  if (rows.length !== 1 || Number(rows[0].one) !== 1) {
    throw new Error(`expected one row with one = 1, got ${rows.length} rows`);
  }
}

/**
 * `ctx.org.places()`, `people()` and `assignments()` answer the shapes Store
 * Ops reads. An org with no places, people or assignments passes; the
 * per-item checks need one of each to bite.
 */
async function orgRead(run: RunState): Promise<void> {
  const { org } = run.ctx;
  const places = listOf(await org.places(), "places");
  const badPlace = places.findIndex((place) => !isPlace(place));
  if (badPlace >= 0) {
    throw new Error(`place ${badPlace} lacks id, name, parent_id, status, timezone or external_ids`);
  }
  const people = listOf(await org.people(), "people");
  const badPerson = people.findIndex((person) => !isPerson(person));
  if (badPerson >= 0) throw new Error(`person ${badPerson} lacks id, name or kind`);
  const assignments = listOf(await org.assignments(), "assignments");
  const badAssignment = assignments.findIndex((assignment) => !isAssignment(assignment));
  if (badAssignment >= 0) {
    throw new Error(`assignment ${badAssignment} lacks id, user_id, role_id, role_scope or location_id`);
  }
}

/** The `key` list of a `{ <key>: [...], total }` answer, or a throw naming the shape. */
function listOf(result: unknown, key: string): unknown[] {
  if (!isRecord(result) || !Array.isArray(result[key]) || typeof result.total !== "number") {
    throw new Error(`ctx.org.${key}() did not resolve { ${key}: [...], total }`);
  }
  return result[key] as unknown[];
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

function isPerson(value: unknown): boolean {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.name === "string" &&
    (value.kind === "member" || value.kind === "frontline")
  );
}

function isAssignment(value: unknown): boolean {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.user_id === "string" &&
    typeof value.role_id === "string" &&
    typeof value.role_scope === "string" &&
    (value.location_id === null || typeof value.location_id === "string")
  );
}

// ── oltp ─────────────────────────────────────────────────────────────────────

/** Created on first use; `run` is the primary key, so a tag is written once. */
async function ensureOltpTable(run: RunState): Promise<void> {
  if (run.oltpTableReady) return;
  await run.ctx.oltp.exec(
    `CREATE TABLE IF NOT EXISTS ${OLTP_TABLE} (
       run        text        PRIMARY KEY,
       written_at timestamptz NOT NULL DEFAULT now()
     )`
  );
  run.oltpTableReady = true;
}

/** Write, read and delete one row in the app's own OLTP schema. A failed delete fails the step. */
async function oltpRoundtrip(run: RunState): Promise<void> {
  const { oltp } = run.ctx;
  await ensureOltpTable(run);
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

/**
 * `ctx.oltp.tx`: a commit lands and a rollback does not. One transaction
 * inserts a row tagged `<runId>-tx` and reads it back on its own connection;
 * after it resolves, a plain `ctx.oltp.query` must see the row. A second
 * inserts `<runId>-rb` and throws; the runtime must roll it back and rethrow
 * that error, and the row must not be there. The committed row is deleted at
 * the end, and a failed delete fails the step.
 */
async function oltpTransaction(run: RunState): Promise<void> {
  const oltp = run.ctx.oltp as OltpWithTx;
  await ensureOltpTable(run);
  const committed = `${run.runId}-tx`;
  const rolledBack = `${run.runId}-rb`;
  const seen = await oltp.tx(async (tx) => {
    await tx.exec(`INSERT INTO ${OLTP_TABLE} (run) VALUES ($1)`, [committed]);
    const rows = await tx.query(`SELECT run FROM ${OLTP_TABLE} WHERE run = $1`, [committed]);
    return rows.length;
  });
  await withCleanup(
    async () => {
      if (seen !== 1) {
        throw new Error(`the transaction read back ${seen} rows of its own insert, expected 1`);
      }
      await expectOltpRows(run, committed, 1, "after commit");
      const abort = new Error("canary: roll this back");
      let thrown: unknown;
      try {
        await oltp.tx(async (tx) => {
          await tx.exec(`INSERT INTO ${OLTP_TABLE} (run) VALUES ($1)`, [rolledBack]);
          throw abort;
        });
      } catch (err) {
        thrown = err;
      }
      if (thrown === undefined) throw new Error("ctx.oltp.tx resolved although the callback threw");
      if (thrown !== abort) {
        throw new Error(`ctx.oltp.tx rethrew something other than the callback's error: ${causeOf(thrown)}`);
      }
      await expectOltpRows(run, rolledBack, 0, "after rollback");
    },
    async () => {
      // The rolled-back tag is included so a rollback that did not roll back
      // leaves nothing behind either; the step has already failed on it.
      const deleted = await oltp.exec(`DELETE FROM ${OLTP_TABLE} WHERE run = $1 OR run = $2`, [
        committed,
        rolledBack
      ]);
      if (deleted === 0) throw new Error("cleanup deleted 0 rows, expected the committed row");
    }
  );
}

async function expectOltpRows(
  run: RunState,
  tag: string,
  expected: number,
  when: string
): Promise<void> {
  const rows = await run.ctx.oltp.query(`SELECT run FROM ${OLTP_TABLE} WHERE run = $1`, [tag]);
  if (rows.length !== expected) {
    throw new Error(`${when}: ${rows.length} rows tagged ${tag}, expected ${expected}`);
  }
}

// ── storage ──────────────────────────────────────────────────────────────────

/**
 * Put and get binary as base64; upload through a presigned PUT and head it;
 * copy the object and read the copy back through a presigned GET; list under
 * `canary/`, walking pages, and expect all three; delete all three.
 */
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

  // `copy` reads the source and writes the destination, the one op gated on
  // both storage capabilities; the copy is read back the way a browser would,
  // through a presigned GET.
  const copy = await storage.copy(put.key, `canary/${run.runId}-copy.bin`);
  written.push(copy.key);
  if (copy.size !== bytes.length) {
    throw new Error(`copy reported ${copy.size} bytes, expected ${bytes.length}`);
  }
  const download = await storage.getDownloadUrl(copy.key);
  const fetched = await run.ctx.fetch(download.url, { encoding: "base64" });
  if (fetched.status < 200 || fetched.status >= 300) {
    throw new Error(`presigned GET answered HTTP ${fetched.status}`);
  }
  if (!sameBytes(base64ToBytes(fetched.body), bytes)) {
    throw new Error("presigned GET returned different bytes than put wrote");
  }

  // Listed under the app-relative directory every pathname above shares —
  // `canary/`, not the run id: a run id is a partial segment, and the local
  // storage backend lists a prefix as a directory while S3 lists it as a key
  // prefix, so only a directory boundary lists the same on both. The keys it
  // returns are the silo keys put and copy returned.
  const listed = await listUnder(storage, "canary/");
  const unlisted = written.filter((key) => !listed.has(key));
  if (unlisted.length > 0) {
    throw new Error(
      `list under canary/ lacks ${unlisted.length} of the ${written.length} objects this run wrote`
    );
  }
}

/**
 * Every key under `prefix`, cursor-paginated so objects an earlier failed run
 * left behind cannot push this run's past the first page; capped at
 * `MAX_LIST_PAGES`.
 */
async function listUnder(
  storage: OxyFunctionContext["storage"],
  prefix: string
): Promise<Set<string>> {
  const keys = new Set<string>();
  let cursor: string | undefined;
  for (let page = 0; page < MAX_LIST_PAGES; page++) {
    const result = await storage.list({ prefix, limit: LIST_PAGE_SIZE, cursor });
    for (const object of result.objects) keys.add(object.key);
    if (!result.hasMore || result.cursor === null) return keys;
    cursor = result.cursor;
  }
  throw new Error(
    `more than ${MAX_LIST_PAGES * LIST_PAGE_SIZE} objects under ${prefix}: earlier runs are not cleaning up`
  );
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
