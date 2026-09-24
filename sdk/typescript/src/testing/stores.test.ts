// The stores evaluate the SQL subset, render typed values through the zoo,
// and say where every row came from.

import { describe, expect, it } from "vitest";
import { REFUSALS } from "./host-contract";
import { literal, parse, splitTopLevel } from "./sql";
import { TableStore, ZooRefusalError } from "./stores";
import { renderValue, ZOO, zooColumn, zooTableName } from "./zoo";

describe("the SQL subset", () => {
  it("splits on top-level commas only", () => {
    expect(splitTopLevel("a, f(b, c), 'x, y', [1, 2]")).toEqual([
      "a",
      "f(b, c)",
      "'x, y'",
      "[1, 2]"
    ]);
  });

  it("reads CREATE TABLE with ClickHouse's trailer and Postgres's constraints", () => {
    const ch = parse(
      `CREATE TABLE IF NOT EXISTS t (\n  run String,\n  path LowCardinality(String),\n  written_at DateTime DEFAULT now()\n) ENGINE = MergeTree ORDER BY (written_at, run) TTL written_at + INTERVAL 7 DAY`
    );
    expect(ch).toMatchObject({
      kind: "create",
      table: "t",
      ifNotExists: true,
      columns: [
        { name: "run", type: "String" },
        { name: "path", type: "LowCardinality(String)" },
        { name: "written_at", type: "DateTime" }
      ]
    });
    const pg = parse("CREATE TABLE r (id int8 NOT NULL, amount numeric, PRIMARY KEY (id))");
    expect(pg).toMatchObject({
      columns: [
        { name: "id", type: "int8" },
        { name: "amount", type: "numeric" }
      ]
    });
  });

  it("keeps each VALUES entry as raw SQL text, comment-headed statements included", () => {
    const s = parse("-- a comment\nINSERT INTO t (a, b) VALUES ('x', toInt128('-1')), (2, NULL)");
    expect(s).toMatchObject({
      kind: "insert",
      table: "t",
      columns: ["a", "b"],
      tuples: [
        ["'x'", "toInt128('-1')"],
        ["2", "NULL"]
      ]
    });
  });

  it("reads SELECT with projections, WHERE, ORDER BY and LIMIT, and a bare literal SELECT", () => {
    expect(
      parse(
        "SELECT path, run AS r FROM t WHERE run = 'k' AND n IN (1, 2) ORDER BY path DESC LIMIT 5"
      )
    ).toEqual({
      kind: "select",
      table: "t",
      items: [
        { kind: "column", column: "path", alias: "path" },
        { kind: "column", column: "run", alias: "r" }
      ],
      where: [
        { column: "run", values: ["'k'"] },
        { column: "n", values: ["1", "2"] }
      ],
      orderBy: { column: "path", desc: true },
      limit: 5
    });
    expect(parse("SELECT count(*) AS n FROM t")).toMatchObject({
      items: [{ kind: "count", alias: "n" }]
    });
    expect(parse("SELECT 1 AS one")).toMatchObject({
      table: null,
      items: [{ kind: "literal", raw: "1", alias: "one" }]
    });
  });

  it("refuses what it cannot evaluate, in the context's own words", () => {
    expect(() => parse("SELECT * FROM a JOIN b ON a.id = b.id")).toThrow(
      /cannot evaluate this statement/
    );
    expect(() => parse("SELECT * FROM t WHERE x > 1")).toThrow(/cannot evaluate this WHERE clause/);
    expect(() => parse("TRUNCATE t")).toThrow(/t\.override\(op/);
  });

  it("refuses GROUP BY rather than dropping it", () => {
    // It used to parse — an optional branch nothing read — so this answered one
    // ungrouped row with no `status` column, which reads exactly like a right
    // answer. Both spellings, because a WHERE changes which arm swallows it.
    expect(() => parse("SELECT status, count(*) FROM orders GROUP BY status")).toThrow(
      /cannot evaluate GROUP BY/
    );
    expect(() => parse("SELECT a FROM t WHERE x = 1 GROUP BY a")).toThrow(/cannot evaluate/);
  });

  it("refuses a projection it cannot read, instead of answering with the SQL", () => {
    // `literal()` hands unrecognised text back unchanged, so these used to
    // return a column holding its own expression: total: "sum(amount)".
    expect(() => parse("SELECT sum(amount) AS total FROM orders")).toThrow(
      /cannot evaluate this statement/
    );
    expect(() => parse("SELECT DISTINCT a FROM t")).toThrow(/cannot evaluate this statement/);
    expect(() => parse("SELECT lower(name) FROM t")).toThrow(/cannot evaluate this statement/);
  });

  it("refuses an assignment it cannot read, and reads GROUP BY as SQL not data", () => {
    // The read side got the guard first; `UPDATE … SET` runs the same
    // `literal()` fall-through, so `n + 1` used to be stored as the string
    // "n + 1" — in a numeric column, on a test that then passed.
    expect(() => parse("UPDATE t SET n = n + 1")).toThrow(/cannot evaluate this statement/);
    expect(() => parse("UPDATE t SET at = now()")).toThrow(/cannot evaluate this statement/);
    expect(parse("UPDATE t SET n = 2 WHERE k = 'a'")).toMatchObject({
      set: [{ column: "n", raw: "2" }]
    });
    // And the GROUP BY refusal reads the statement, not a row's contents.
    expect(parse("SELECT a FROM t WHERE label = 'GROUP BY'")).toMatchObject({ kind: "select" });
  });

  it("still reads the literal shapes it can", () => {
    // The refusal above must not swallow what `literal()` genuinely handles.
    expect(parse("SELECT 1 AS one, 'a' AS letter, NULL AS nil, TRUE AS yes")).toMatchObject({
      items: [
        { kind: "literal", raw: "1", alias: "one" },
        { kind: "literal", raw: "'a'", alias: "letter" },
        { kind: "literal", raw: "NULL", alias: "nil" },
        { kind: "literal", raw: "TRUE", alias: "yes" }
      ]
    });
    expect(parse("SELECT $1 AS bound")).toMatchObject({
      items: [{ kind: "literal", raw: "$1", alias: "bound" }]
    });
  });

  it("reads literals and bound parameters", () => {
    expect(literal("'it''s'")).toBe("it's");
    expect(literal("-1.25")).toBe(-1.25);
    expect(literal("NULL")).toBeNull();
    expect(literal("$2", ["a", "b"])).toBe("b");
    expect(literal("now()")).toBe("now()");
  });
});

describe("TableStore", () => {
  it("creates, inserts, reads back with WHERE and ORDER BY, deletes and updates", () => {
    const store = new TableStore("clickhouse", "warehouse");
    store.exec("CREATE TABLE IF NOT EXISTS w (run String, path String)");
    expect(
      store.exec("INSERT INTO w (run, path) VALUES ('r1', 'b'), ('r1', 'a'), ('r2', 'c')")
    ).toBe(3);
    expect(store.query("SELECT path FROM w WHERE run = 'r1' ORDER BY path").rows).toEqual([
      { path: "a" },
      { path: "b" }
    ]);
    expect(store.query("SELECT count(*) AS n FROM w").rows).toEqual([{ n: 3 }]);
    expect(store.exec("UPDATE w SET path = 'z' WHERE run = 'r2'")).toBe(1);
    expect(store.exec("DELETE FROM w WHERE run = 'r1'")).toBe(2);
    expect(store.rows("w")).toEqual([{ run: "r2", path: "z" }]);
  });

  it("binds $n parameters on the OLTP plane", () => {
    const store = new TableStore("postgres", "oltp", "app_x");
    store.exec("CREATE TABLE app_x.r (tag text, n int4)");
    store.exec("INSERT INTO r (tag, n) VALUES ($1, $2)", ["k", 7]);
    expect(store.query("SELECT n FROM app_x.r WHERE tag = $1", ["k"]).rows).toEqual([{ n: 7 }]);
  });

  it("renders a typed column exactly when the value is a zoo case, by shape otherwise", () => {
    const store = new TableStore("clickhouse", "warehouse");
    store.table("s", { id: "UInt64", name: "String", ok: "Bool" }).insert([
      { id: "18446744073709551615", name: "'plain text'", ok: "true" },
      { id: "42", name: "'other'", ok: "false" },
      { id: "9999999999999999999", name: "NULL", ok: "1" }
    ]);
    const read = store.query("SELECT id, name, ok FROM s");
    expect(read.rows).toEqual([
      { id: "18446744073709551615", name: "plain text", ok: true },
      { id: 42, name: "other", ok: false },
      { id: "9999999999999999999", name: null, ok: true }
    ]);
    expect(read.source).toBe("typed");
    expect(store.query("SELECT id FROM s WHERE id = '18446744073709551615'").source).toBe("zoo");
  });

  it("refuses a type the zoo does not pin, at declaration", () => {
    const store = new TableStore("postgres", "warehouse");
    expect(() => store.table("x", { c: "money" })).toThrow(/has no postgres case for type "money"/);
  });

  it("marks author rows, and the weakest source wins on a mixed read", () => {
    const store = new TableStore("duckdb", "warehouse");
    store.table("m", { n: "HUGEINT" }).insert([{ n: "42" }]);
    expect(store.query("SELECT n FROM m").rows).toEqual([{ n: "42" }]);
    store.raw("m", [{ n: 7 }]);
    const read = store.query("SELECT n FROM m");
    expect(read.rows).toEqual([{ n: "42" }, { n: 7 }]);
    expect(read.source).toBe("author");
  });

  it("answers the zoo table per engine, and throws the host's sentence for an OLTP $error case", () => {
    const warehouse = new TableStore("clickhouse", "warehouse").zoo();
    const cases = ZOO.engines.clickhouse.cases;
    const wide = cases.findIndex((c) => c.key === "clickhouse/UInt64/precision_gt_18");
    expect(warehouse.query(`SELECT count(*) AS n FROM ${zooTableName()}`).rows).toEqual([{ n: 1 }]);
    expect(
      warehouse.query(`SELECT ${zooColumn(wide)} FROM ${zooTableName()} LIMIT 1`).rows
    ).toEqual([{ [zooColumn(wide)]: "18446744073709551615" }]);
    expect(warehouse.exec(`CREATE TABLE IF NOT EXISTS ${zooTableName()} (c001 String)`)).toBe(0);

    const oltp = new TableStore("postgres", "oltp").zoo();
    const numeric = ZOO.engines.postgres.cases.findIndex((c) => c.key === "postgres/numeric/plain");
    const column = zooColumn(numeric);
    let thrown: unknown;
    try {
      oltp.query(`SELECT ${column} FROM ${zooTableName()} LIMIT 1`);
    } catch (err) {
      thrown = err;
    }
    expect(thrown).toBeInstanceOf(ZooRefusalError);
    expect((thrown as Error).message).toBe(
      REFUSALS.oltpUnsupportedColumn.refusal.replace("{name}", column).replace("{ty}", "numeric")
    );
  });

  it("snapshots and restores for a rollback", () => {
    const store = new TableStore("postgres", "warehouse");
    store.raw("t", [{ a: 1 }]);
    const restore = store.snapshot();
    store.raw("t", [{ a: 2 }]);
    expect(store.rows("t")).toHaveLength(2);
    restore();
    expect(store.rows("t")).toEqual([{ a: 1 }]);
  });
});

describe("renderValue", () => {
  it("pins the shapes the spec tables", () => {
    const v = (
      engine: "clickhouse" | "postgres" | "duckdb",
      type: string,
      sql: string,
      plane: "warehouse" | "oltp" = "warehouse"
    ) => renderValue(engine, type, sql, plane, "c").value;
    expect(v("clickhouse", "Int128", "toInt128('-170141183460469231731687303715884105728')")).toBe(
      "-170141183460469231731687303715884105728"
    );
    expect(v("duckdb", "HUGEINT", "42")).toBe("42");
    expect(v("postgres", "int8", "9223372036854775807")).toBe(9223372036854776000);
    expect(v("duckdb", "INTEGER[]", "[1, -2, 3]")).toBe("List([Int(1), Int(-2), Int(3)])");
    expect(v("postgres", "timestamptz", "TIMESTAMPTZ '2024-03-10 12:34:56.789+00'")).toBe(
      "2024-03-10 12:34:56.789000"
    );
    expect(v("postgres", "timestamptz", "TIMESTAMPTZ '2024-03-10 12:34:56.789+00'", "oltp")).toBe(
      "2024-03-10T12:34:56.789000Z"
    );
    expect(v("clickhouse", "Float64", "inf")).toBeNull();
  });
});
