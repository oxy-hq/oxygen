/// <reference types="node" />
// The canary's zoo copy and digest match fixtures/data-shapes/zoo.json at the repo root, and the
// SQL rules match the Rust loader's shared vector (crates/app/tests/custom_apps/shape_zoo.rs,
// `SAMPLE`). Only this test reads outside the app, by relative path; the bundle carries the copy.
// The `node` reference is needed because functions/tsconfig.json sets `types: []`.

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  countSql,
  createTableSql,
  insertSql,
  type ShapeZoo,
  selectColumnSql,
  zooTableName
} from "./shape-zoo";
import { ZOO_SHA256 } from "./shape-zoo-sha256";

const FIXTURE = new URL("../../../../fixtures/data-shapes/zoo.json", import.meta.url);
const COPY = new URL("./shape-zoo.json", import.meta.url);
const SYNC = "run `node scripts/data-shapes/sync-canary-zoo.mjs` from the repo root";

function sha256(bytes: Uint8Array | string): string {
  return createHash("sha256").update(bytes).digest("hex");
}

/** The same 783 bytes as `SAMPLE` in shape_zoo.rs. */
const SAMPLE =
  '{"version":1,"engines":{"clickhouse":{"cases":[{"key":"clickhouse/String/non_bmp","native_type":"String","class":"non_bmp","value_sql":"\'𝔘nicode\'","expect":{"warehouse":"𝔘nicode"}},{"key":"clickhouse/Nullable(Int32)/null","native_type":"Nullable(Int32)","class":"null","value_sql":"NULL","expect":{"warehouse":null}}]},"postgres":{"cases":[{"key":"postgres/numeric/negative","native_type":"numeric","class":"negative","value_sql":"-1.25","expect":{"warehouse":"-1.25","oltp":{"$error":"which cannot be returned directly"}}},{"key":"postgres/timestamptz/tz_bearing","native_type":"timestamptz","class":"tz_bearing","value_sql":"TIMESTAMPTZ \'2024-03-10 12:34:56+05:30\'","expect":{"warehouse":"2024-03-10 07:04:56","oltp":"2024-03-10T07:04:56.000000Z"}}]},"duckdb":{"cases":[]}}}' +
  "\n";

describe("the canary's shape zoo", () => {
  it("is a byte copy of fixtures/data-shapes/zoo.json", () => {
    expect(readFileSync(COPY).equals(readFileSync(FIXTURE)), `stale copy: ${SYNC}`).toBe(true);
  });

  it("carries the fixture's SHA-256", () => {
    expect(ZOO_SHA256, `stale digest: ${SYNC}`).toBe(sha256(readFileSync(FIXTURE)));
  });
});

describe("the SQL shared with the Rust loader", () => {
  const zoo = JSON.parse(SAMPLE) as ShapeZoo;
  const table = zooTableName(sha256(SAMPLE));

  it("names the table from the file's digest", () => {
    expect(sha256(SAMPLE)).toBe("a7a68bfb136327ec9d55f27de1be43c71f78694b35c7b6943d3587070854b0c2");
    expect(table).toBe("oxy_shape_zoo_a7a68bfb");
  });

  it("builds the ClickHouse DDL and insert", () => {
    const cases = zoo.engines.clickhouse.cases;
    expect(createTableSql("clickhouse", table, cases)).toBe(
      "CREATE TABLE IF NOT EXISTS oxy_shape_zoo_a7a68bfb (c001 String, c002 Nullable(Int32)) ENGINE = MergeTree ORDER BY tuple()"
    );
    expect(insertSql(table, cases)).toBe(
      "INSERT INTO oxy_shape_zoo_a7a68bfb VALUES ('𝔘nicode', NULL)"
    );
  });

  it("builds the Postgres DDL and insert, the count and a column read", () => {
    const cases = zoo.engines.postgres.cases;
    expect(createTableSql("postgres", table, cases)).toBe(
      "CREATE TABLE IF NOT EXISTS oxy_shape_zoo_a7a68bfb (c001 numeric, c002 timestamptz)"
    );
    expect(insertSql(table, cases)).toBe(
      "INSERT INTO oxy_shape_zoo_a7a68bfb VALUES (-1.25, TIMESTAMPTZ '2024-03-10 12:34:56+05:30')"
    );
    expect(selectColumnSql(table, 1)).toBe("SELECT c002 FROM oxy_shape_zoo_a7a68bfb LIMIT 1");
    expect(countSql(table)).toBe("SELECT count(*) AS n FROM oxy_shape_zoo_a7a68bfb");
  });
});
