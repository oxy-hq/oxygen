// shape-zoo: the platform canary's shape zoo, and the SQL and checks built from it.
//
// shape-zoo.json is a byte copy of fixtures/data-shapes/zoo.json, and shape-zoo-sha256.ts holds
// its digest. scripts/data-shapes/sync-canary-zoo.mjs writes both. The table and SQL rules match
// crates/app/tests/custom_apps/shape_zoo.rs, and both sides test one shared vector
// (shape-zoo-sync.test.ts). The values are synthetic, and a failure names the case key and class.
//
// Types only from `@oxy-hq/sdk`: a value import fails the isolate at load (see steps.ts).

import type { OxyFunctionContext, OxyFunctionRow } from "@oxy-hq/sdk";
import zooJson from "./shape-zoo.json";
import { ZOO_SHA256 } from "./shape-zoo-sha256";

export type ZooEngine = "clickhouse" | "postgres" | "duckdb";
export type ZooPlaneName = "warehouse" | "oltp";

export interface ZooCase {
  key: string;
  native_type: string;
  class: string;
  value_sql: string;
  expect: { warehouse: unknown; oltp?: unknown };
}

export interface ShapeZoo {
  version: number;
  engines: Record<ZooEngine, { cases: ZooCase[] }>;
}

/** Where a zoo table lives: `ctx.warehouse` on one database, or `ctx.oltp`. */
export interface ZooPlane {
  exec(sql: string): Promise<unknown>;
  query(sql: string): Promise<OxyFunctionRow[]>;
}

export type ZooRead = { ok: true; value: unknown } | { ok: false; message: string };

const ZOO = zooJson as unknown as ShapeZoo;

export function zooTableName(sha256Hex: string): string {
  return `oxy_shape_zoo_${sha256Hex.slice(0, 8)}`;
}

export function zooColumn(index: number): string {
  return `c${String(index + 1).padStart(3, "0")}`;
}

export function createTableSql(engine: ZooEngine, table: string, cases: ZooCase[]): string {
  const columns = cases.map((c, i) => `${zooColumn(i)} ${c.native_type}`).join(", ");
  const ddl = `CREATE TABLE IF NOT EXISTS ${table} (${columns})`;
  return engine === "clickhouse" ? `${ddl} ENGINE = MergeTree ORDER BY tuple()` : ddl;
}

export function insertSql(table: string, cases: ZooCase[]): string {
  return `INSERT INTO ${table} VALUES (${cases.map((c) => c.value_sql).join(", ")})`;
}

export function countSql(table: string): string {
  return `SELECT count(*) AS n FROM ${table}`;
}

export function selectColumnSql(table: string, index: number): string {
  return `SELECT ${zooColumn(index)} FROM ${table} LIMIT 1`;
}

/**
 * Create the table, and fill its one row only when it has none. Two runs can both see an empty
 * table and both insert; the rows are identical, and every read takes `LIMIT 1`.
 */
export async function loadZoo(
  plane: ZooPlane,
  engine: ZooEngine,
  table: string,
  cases: ZooCase[]
): Promise<void> {
  await plane.exec(createTableSql(engine, table, cases));
  const [counted] = await plane.query(countSql(table));
  if (Number(counted?.n ?? 0) === 0) await plane.exec(insertSql(table, cases));
}

/** Read each case's column, one statement per column, and throw at the first that differs. */
export async function checkZoo(
  plane: ZooPlane,
  planeName: ZooPlaneName,
  table: string,
  cases: ZooCase[]
): Promise<void> {
  for (const [index, zooCase] of cases.entries()) {
    const read = await readColumn(plane, table, index);
    const mismatch = describeMismatch(zooCase, zooCase.expect[planeName], read);
    if (mismatch) throw new Error(mismatch);
  }
}

/**
 * ClickHouse cases through `ctx.warehouse` on `database`, then Postgres cases through `ctx.oltp`.
 * A passing run logs one line, `shape_zoo: <cases> cases in <ms> ms`, so the invocation's log
 * reads against `canary`'s 180-second budget. No case value is logged. The line is `info`, so
 * it is in the app's `/logs` and the job's `/function-runs/<run_id>`, not in the pod logs.
 */
export async function runShapeZoo(
  ctx: OxyFunctionContext,
  database: string,
  zoo: ShapeZoo = ZOO,
  sha256: string = ZOO_SHA256
): Promise<void> {
  if (zoo.version !== 1) throw new Error(`shape zoo version ${zoo.version} is not 1`);
  const started = Date.now();
  const table = zooTableName(sha256);
  const warehouse: ZooPlane = {
    exec: (sql) => ctx.warehouse.exec(database, sql),
    query: async (sql) => (await ctx.warehouse.query(database, sql)).rows
  };
  const oltp: ZooPlane = {
    exec: (sql) => ctx.oltp.exec(sql),
    query: (sql) => ctx.oltp.query(sql)
  };
  const clickhouse = zoo.engines.clickhouse.cases;
  await loadZoo(warehouse, "clickhouse", table, clickhouse);
  await checkZoo(warehouse, "warehouse", table, clickhouse);
  const postgres = zoo.engines.postgres.cases;
  await loadZoo(oltp, "postgres", table, postgres);
  await checkZoo(oltp, "oltp", table, postgres);
  const cases = clickhouse.length + postgres.length;
  ctx.log(`shape_zoo: ${cases} cases in ${Date.now() - started} ms`);
}

/** `undefined` when the read matches; otherwise `<key> (<class>): expected <json> got <json>`. */
export function describeMismatch(
  zooCase: ZooCase,
  expected: unknown,
  read: ZooRead
): string | undefined {
  const needle = errorNeedle(expected);
  const matched =
    needle !== undefined
      ? !read.ok && read.message.includes(needle)
      : read.ok && sameJson(expected, read.value);
  if (matched) return undefined;
  const got = read.ok ? read.value : { $error: read.message };
  return `${zooCase.key} (${zooCase.class}): expected ${JSON.stringify(expected)} got ${JSON.stringify(got)}`;
}

async function readColumn(plane: ZooPlane, table: string, index: number): Promise<ZooRead> {
  try {
    const rows = await plane.query(selectColumnSql(table, index));
    if (rows.length !== 1) return { ok: false, message: `expected 1 row, got ${rows.length}` };
    return { ok: true, value: rows[0][zooColumn(index)] };
  } catch (err) {
    return { ok: false, message: err instanceof Error ? err.message : String(err) };
  }
}

/** `{"$error": needle}`: the case expects this read to be refused with a message containing it. */
function errorNeedle(value: unknown): string | undefined {
  if (!isRecord(value) || Object.keys(value).length !== 1) return undefined;
  return typeof value.$error === "string" ? value.$error : undefined;
}

/** JSON equality as the isolate sees it: numbers are already doubles, so `===` compares them. */
function sameJson(expected: unknown, actual: unknown): boolean {
  if (Array.isArray(expected)) {
    return (
      Array.isArray(actual) &&
      expected.length === actual.length &&
      expected.every((item, i) => sameJson(item, actual[i]))
    );
  }
  if (isRecord(expected)) {
    if (!isRecord(actual)) return false;
    const keys = Object.keys(expected);
    return (
      keys.length === Object.keys(actual).length &&
      keys.every((k) => k in actual && sameJson(expected[k], actual[k]))
    );
  }
  return expected === actual;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
