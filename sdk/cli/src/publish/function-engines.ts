/**
 * The half of the Oxy Functions lint that needs the server: what KIND of
 * store each `ctx.warehouse` / `ctx.tx` write lands on, and which engine.
 *
 * The manifest's `destinations` name workspace databases, not their engines,
 * so `oxyc publish` asks the target — `GET /api/{project}/databases`, the
 * `list_databases` route `oxyc routes databases` lists, which answers `name`,
 * `dialect` (the query engine: `postgres`, `duckdb`, `clickhouse`, …) and
 * `db_type` (the `config.yml` kind: `airhouse_managed`, `postgres_managed`,
 * `snowflake`, …). Three refusals the host makes are then answerable before
 * the upload:
 *
 * - `customer-warehouse` — `host/destinations.rs` `destination_write_policy`:
 *   Airhouse (`airhouse` / `airhouse_managed`) takes the write; the org's OLTP
 *   (`postgres_managed`) is refused with a pointer at `ctx.oltp`; anything else
 *   is a customer warehouse, read-only unless the function names it in
 *   `customerWarehouseWrites` with a reason.
 * - `engine` for `ctx.warehouse.upsert` — `upsert_support.rs`: it compiles to
 *   `INSERT … ON CONFLICT`, which only the `postgres` and `duckdb` dialects
 *   parse, so every other dialect is refused by name.
 * - `engine` for `ctx.tx` — Postgres only; another backend rejects the
 *   transaction rather than faking it statement by statement.
 *
 * SKIPPED, NEVER GUESSED, when the list cannot be fetched: an unreachable or
 * unauthorised target prints one warning and the publish goes on, because the
 * host still makes every one of these refusals at the first call.
 */

import { CliError, ExitCode } from "../util/errors.js";
import { GATED_CAPABILITIES } from "./capabilities.js";
import { type CtxCall, describeLintIssue, type FunctionLintIssue } from "./function-lint.js";

/** The fields of `DatabaseInfo` this reads; the route answers more. */
export interface DatabaseEngine {
  name: string;
  dialect: string;
  db_type: string;
}

const LOOKUP_TIMEOUT_MS = 30_000;

/** Dialects whose `INSERT … ON CONFLICT` the host lets through (`parses_on_conflict`). */
export const UPSERT_DIALECTS: readonly string[] = ["postgres", "duckdb"];
/** The one dialect `ctx.tx` opens a transaction on. */
export const TX_DIALECTS: readonly string[] = ["postgres"];
/** `destination_kind`: these take a write with no exception. */
const AIRHOUSE_TYPES: readonly string[] = ["airhouse", "airhouse_managed"];
const MANAGED_OLTP_TYPE = "postgres_managed";

/**
 * The project's databases, or `undefined` with the reason when the target did
 * not answer — a network failure, a 401/403, a body that is not the list. The
 * caller prints the reason once and continues: this check is advisory, and a
 * publish must not fail on a lookup the upload itself does not need.
 */
export async function fetchDatabaseEngines(
  target: string,
  project: string,
  token: string
): Promise<{ databases: DatabaseEngine[] } | { skipped: string }> {
  const url = `${target.replace(/\/+$/, "")}/api/${encodeURIComponent(project)}/databases`;
  let response: Response;
  try {
    response = await fetch(url, {
      headers: { authorization: `Bearer ${token}` },
      signal: AbortSignal.timeout(LOOKUP_TIMEOUT_MS)
    });
  } catch (cause) {
    return { skipped: `GET ${url} failed: ${(cause as Error).message}` };
  }
  if (!response.ok) return { skipped: `GET ${url} answered ${response.status}` };
  let parsed: unknown;
  try {
    parsed = JSON.parse(await response.text());
  } catch {
    return { skipped: `GET ${url} did not answer JSON` };
  }
  if (!Array.isArray(parsed)) return { skipped: `GET ${url} did not answer a list` };
  const databases = parsed.filter(
    (d): d is DatabaseEngine =>
      typeof d === "object" &&
      d !== null &&
      typeof (d as DatabaseEngine).name === "string" &&
      typeof (d as DatabaseEngine).dialect === "string" &&
      typeof (d as DatabaseEngine).db_type === "string"
  );
  // A row this cannot read is a database this cannot see, and "not a database
  // of this project" would then be a refusal built on a guess — so the whole
  // list is skipped, the same as one that never arrived.
  if (databases.length !== parsed.length) {
    return {
      skipped:
        `GET ${url} answered ${parsed.length - databases.length} row(s) without a string ` +
        "name, dialect and db_type"
    };
  }
  return { databases };
}

/** The gates a write's database decides — one today, and never silently a second. */
const DESTINATION_GATES = GATED_CAPABILITIES.filter((c) => c.gate === "destination");
const DESTINATION_DECLARE = DESTINATION_GATES.map((c) => c.declare).join(" and ");

function issue(call: CtxCall, rule: FunctionLintIssue["rule"], message: string): FunctionLintIssue {
  return { rule, fn: call.fn, level: "error", file: call.file, path: `line ${call.line}`, message };
}

/** `ctx.warehouse.upsert("db", …)` — the call with the database it named. */
const written = (call: CtxCall) => `${call.call}"${call.database}", …)`;

/**
 * Every refusal the engine list settles for the writes `lintAppFunctions`
 * collected. A write whose database is not a literal has nothing to look up
 * and is the host's to judge.
 */
export function checkEngines(
  writes: CtxCall[],
  manifest: Record<string, unknown>,
  databases: DatabaseEngine[]
): FunctionLintIssue[] {
  const issues: FunctionLintIssue[] = [];
  const functions =
    typeof manifest.functions === "object" && manifest.functions !== null
      ? (manifest.functions as Record<string, unknown>)
      : {};
  for (const call of writes) {
    if (call.database === undefined) continue;
    const spec = functions[call.fn];
    if (typeof spec !== "object" || spec === null) continue;
    const db = databases.find((d) => d.name === call.database);
    if (!db) {
      issues.push(
        issue(
          call,
          "destinations",
          `\`${written(call)}\` writes \`${call.database}\`, which is not a database of this ` +
            "project — the host refuses a write to a database that is not configured; check " +
            "the name against the workspace's config.yml"
        )
      );
      continue;
    }
    const kind = kindIssue(call, spec as Record<string, unknown>, db);
    if (kind) issues.push(kind);
    const engine = engineIssue(call, db);
    if (engine) issues.push(engine);
  }
  return issues;
}

function kindIssue(
  call: CtxCall,
  spec: Record<string, unknown>,
  db: DatabaseEngine
): FunctionLintIssue | undefined {
  if (AIRHOUSE_TYPES.includes(db.db_type)) return undefined;
  if (db.db_type === MANAGED_OLTP_TYPE) {
    return issue(
      call,
      "customer-warehouse",
      `\`${written(call)}\` writes \`${db.name}\`, the org's OLTP store, which \`ctx.warehouse\` ` +
        "and `ctx.tx` reach as the read-only analyst — write this app's own records with " +
        "`ctx.oltp` instead (its `app_<writer>` schema needs no destination)"
    );
  }
  if (DESTINATION_GATES.some((gate) => gate.declares(spec, db.name))) return undefined;
  const tx =
    call.member === "tx"
      ? " (a `ctx.tx` transaction counts as a write even when it only reads — read with " +
        "`ctx.warehouse.query` instead)"
      : "";
  return issue(
    call,
    "customer-warehouse",
    `\`${written(call)}\` writes \`${db.name}\` (${db.db_type}), a customer warehouse, and ` +
      "customer warehouses are read-only to apps — facts belong in `ctx.airhouse`, records in " +
      `\`ctx.oltp\`; if this write has to stay, say why: add ${DESTINATION_DECLARE}` +
      ` to functions.${call.fn} in oxy-app.json${tx}`
  );
}

function engineIssue(call: CtxCall, db: DatabaseEngine): FunctionLintIssue | undefined {
  if (call.member === "warehouse.upsert" && !UPSERT_DIALECTS.includes(db.dialect)) {
    return issue(
      call,
      "engine",
      `\`${written(call)}\` — \`${db.name}\` is ${db.dialect}, and \`upsert\` compiles to ` +
        "`INSERT … ON CONFLICT … DO UPDATE`, which only Postgres and DuckDB parse; the host " +
        "refuses it by name — use `ctx.warehouse.insert`, or `ctx.warehouse.exec` with this " +
        "warehouse's own upsert statement"
    );
  }
  if (call.member === "tx" && !TX_DIALECTS.includes(db.dialect)) {
    return issue(
      call,
      "engine",
      `\`${written(call)}\` — \`${db.name}\` is ${db.dialect}, and \`ctx.tx\` is Postgres-only; ` +
        "the host refuses to open the transaction rather than run the statements one by one — " +
        "use `ctx.warehouse.exec` per statement, or `ctx.oltp.tx` for the app's own store"
    );
  }
  return undefined;
}

/**
 * The lint's verdict as an error, for a publish that does not carry
 * `--allow-function-lint`. FAILURE (1), the code `oxyc validate` gives a
 * finding: the command ran and the answer is bad. Not REFUSED (8) — nothing
 * would have been destroyed — and not USAGE (2): the invocation was fine.
 */
export function functionLintFailure(issues: FunctionLintIssue[], prefix = ""): CliError {
  return new CliError(
    `${issues.length} Oxy Function lint problem(s) — the host would refuse these at the first call`,
    {
      code: ExitCode.FAILURE,
      detail: issues.map((issue) => describeLintIssue(issue, prefix)).join("\n"),
      hint: "each line above names the file, the line, the call and the fix",
      remedy:
        "fix the manifest or the call, then publish again. A false positive? " +
        "`oxyc publish --allow-function-lint` publishes with these as warnings — and please " +
        "open an issue naming the rule, so the rule gets fixed"
    }
  );
}
