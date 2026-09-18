// The shape_zoo step against fake planes. The load is idempotent; a mismatch names the case key
// and class; a `$error` case passes only on a refusal carrying its text; a passing run logs its
// case count and elapsed time; and a failure surfaces through runCanary under the step's own name.

import type { OxyFunctionContext, OxyFunctionRow } from "@oxy-hq/sdk";
import { describe, expect, it } from "vitest";
import {
  checkZoo,
  loadZoo,
  runShapeZoo,
  type ShapeZoo,
  type ZooCase,
  type ZooPlane
} from "./shape-zoo";
import { runCanary } from "./steps";

const DECIMAL: ZooCase = {
  key: "clickhouse/Decimal(18, 4)/plain",
  native_type: "Decimal(18, 4)",
  class: "plain",
  value_sql: "1234.5678",
  expect: { warehouse: "1234.5678" }
};
const NULLABLE: ZooCase = {
  key: "clickhouse/Nullable(Int32)/null",
  native_type: "Nullable(Int32)",
  class: "null",
  value_sql: "NULL",
  expect: { warehouse: null }
};
const NUMERIC: ZooCase = {
  key: "postgres/numeric/negative",
  native_type: "numeric",
  class: "negative",
  value_sql: "-1.25",
  expect: { warehouse: "-1.25", oltp: { $error: "which cannot be returned directly" } }
};
const ZOO: ShapeZoo = {
  version: 1,
  engines: {
    clickhouse: { cases: [DECIMAL, NULLABLE] },
    postgres: { cases: [NUMERIC] },
    duckdb: { cases: [] }
  }
};
const SHA256 = `435b9f1e${"0".repeat(56)}`;
const TABLE = "oxy_shape_zoo_435b9f1e";
const REFUSAL =
  "result column `c001` has Postgres type `numeric`, which cannot be returned directly";

/** One table: `values` by column (`fallback` for the rest), `refused` columns throw their text. */
function fakePlane(
  values: Record<string, unknown>,
  refused: Record<string, string> = {},
  fallback: unknown = null
) {
  const execs: string[] = [];
  let rows = 0;
  const plane: ZooPlane = {
    exec: async (sql) => {
      execs.push(sql);
      if (sql.startsWith("INSERT")) rows += 1;
      return 0;
    },
    query: async (sql): Promise<OxyFunctionRow[]> => {
      if (sql.startsWith("SELECT count(*)")) return [{ n: rows }];
      const column = /^SELECT (c\d{3}) FROM/.exec(sql)?.[1] ?? "";
      if (column in refused) throw new Error(refused[column]);
      return [{ [column]: column in values ? values[column] : fallback }];
    }
  };
  return { plane, execs };
}

/**
 * A ctx whose warehouse and oltp are fake planes, recording which database each call named and
 * every `ctx.log` call's arguments.
 */
function fakeCtx(warehouseValues: Record<string, unknown>, fallback: unknown = null) {
  const databases: string[] = [];
  const logs: unknown[][] = [];
  const warehouse = fakePlane(warehouseValues, {}, fallback);
  const oltp = fakePlane({}, { c001: REFUSAL }, fallback);
  const ctx = {
    log: (...args: unknown[]) => {
      logs.push(args);
    },
    warehouse: {
      exec: async (database: string, sql: string) => {
        databases.push(database);
        return warehouse.plane.exec(sql);
      },
      query: async (database: string, sql: string) => {
        databases.push(database);
        return { rows: await warehouse.plane.query(sql), truncated: false };
      }
    },
    oltp: {
      exec: (sql: string) => oltp.plane.exec(sql),
      query: (sql: string) => oltp.plane.query(sql)
    }
  } as unknown as OxyFunctionContext;
  return { ctx, databases, logs, warehouseExecs: warehouse.execs, oltpExecs: oltp.execs };
}

describe("loadZoo", () => {
  it("creates the table and inserts its row once; a second load only creates", async () => {
    const { plane, execs } = fakePlane({});
    await loadZoo(plane, "clickhouse", TABLE, [DECIMAL]);
    await loadZoo(plane, "clickhouse", TABLE, [DECIMAL]);
    const create =
      "CREATE TABLE IF NOT EXISTS oxy_shape_zoo_435b9f1e (c001 Decimal(18, 4)) ENGINE = MergeTree ORDER BY tuple()";
    expect(execs).toEqual([create, "INSERT INTO oxy_shape_zoo_435b9f1e VALUES (1234.5678)", create]);
  });
});

describe("checkZoo", () => {
  it("passes when every column is its expectation", async () => {
    const { plane } = fakePlane({ c001: "1234.5678", c002: null });
    await expect(checkZoo(plane, "warehouse", TABLE, [DECIMAL, NULLABLE])).resolves.toBeUndefined();
  });

  it("names the key and class, with both values as JSON", async () => {
    const { plane } = fakePlane({ c001: 1234.5678, c002: null });
    await expect(checkZoo(plane, "warehouse", TABLE, [DECIMAL, NULLABLE])).rejects.toThrow(
      'clickhouse/Decimal(18, 4)/plain (plain): expected "1234.5678" got 1234.5678'
    );
  });

  it("passes a $error case only on a refusal that carries its text", async () => {
    const refused = fakePlane({}, { c001: REFUSAL }).plane;
    await expect(checkZoo(refused, "oltp", TABLE, [NUMERIC])).resolves.toBeUndefined();

    const answered = fakePlane({ c001: "-1.25" }).plane;
    await expect(checkZoo(answered, "oltp", TABLE, [NUMERIC])).rejects.toThrow(
      'postgres/numeric/negative (negative): expected {"$error":"which cannot be returned directly"} got "-1.25"'
    );

    const otherError = fakePlane({}, { c001: "connection reset" }).plane;
    await expect(checkZoo(otherError, "oltp", TABLE, [NUMERIC])).rejects.toThrow(
      'got {"$error":"connection reset"}'
    );
  });
});

describe("runShapeZoo", () => {
  it("loads and reads ClickHouse cases on the given database, Postgres cases through ctx.oltp", async () => {
    const fake = fakeCtx({ c001: "1234.5678", c002: null });
    await runShapeZoo(fake.ctx, "canary_warehouse", ZOO, SHA256);
    expect(new Set(fake.databases)).toEqual(new Set(["canary_warehouse"]));
    expect(fake.warehouseExecs[0]).toContain("ENGINE = MergeTree ORDER BY tuple()");
    expect(fake.oltpExecs).toEqual([
      "CREATE TABLE IF NOT EXISTS oxy_shape_zoo_435b9f1e (c001 numeric)",
      "INSERT INTO oxy_shape_zoo_435b9f1e VALUES (-1.25)"
    ]);
  });

  it("logs one line with the case count and elapsed milliseconds, and no values", async () => {
    const fake = fakeCtx({ c001: "1234.5678", c002: null });
    await runShapeZoo(fake.ctx, "canary_warehouse", ZOO, SHA256);
    expect(fake.logs).toEqual([[expect.stringMatching(/^shape_zoo: 3 cases in \d+ ms$/)]]);
  });

  it("logs nothing when a case fails", async () => {
    const fake = fakeCtx({}, "wrong");
    await expect(runShapeZoo(fake.ctx, "canary_warehouse", ZOO, SHA256)).rejects.toThrow();
    expect(fake.logs).toEqual([]);
  });

  it("fails the canary as `canary step shape_zoo failed: <key> (<class>): …`", async () => {
    const fake = fakeCtx({}, "wrong");
    await expect(
      runCanary(fake.ctx, { runId: "run-k3x9a1", steps: ["shape_zoo"] })
    ).rejects.toThrow(/^canary step shape_zoo failed: clickhouse\/\S+ \(\w+\): expected .+ got "wrong"$/);
  });
});
