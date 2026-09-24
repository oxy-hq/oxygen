// `createTestContext` against a manifest shaped like the platform canary's:
// the gates refuse in the host's words, the calls are recorded under the
// host's op names, `override` replaces a host op, `ctx.tx` follows `__runTx`,
// and reads come from the zoo. The worked example of
// `internal-docs/sdk-testing-context.md` §8, on the context itself.

import { describe, expect, it } from "vitest";
import type { OxyFunctionContext } from "../custom-app/function-context";
import { createTestContext } from "./context";
import { HOST_OPS, type HostOp, REFUSALS } from "./host-contract";
import { HOST_ERROR_NAME } from "./host-error";
import { ZOO, zooColumn, zooTableName } from "./zoo";

/** The platform canary's manifest, as `oxy-app.json` declares it. */
const manifest = {
  schemaVersion: 2,
  slug: "platform-canary",
  functions: {
    canary: {
      route: true,
      destinations: ["canary_warehouse"],
      customerWarehouseWrites: {
        canary_warehouse: "The canary exercises the customer-warehouse write path."
      },
      oltp: { enabled: true },
      org: { read: true },
      storage: { read: true, write: true },
      secrets: { write: true }
    },
    echo: { route: true }
  }
};

const DATABASES = {
  canary_warehouse: { dialect: "clickhouse", kind: "customer" },
  appdb: { dialect: "postgres", kind: "customer" },
  facts: { dialect: "duckdb", kind: "airhouse" },
  org_oltp: { dialect: "postgres", kind: "postgres_managed" }
} as const;

const canary = () => createTestContext(manifest, { function: "canary", databases: DATABASES });
const echo = () => createTestContext(manifest, { function: "echo", databases: DATABASES });

async function refusal(call: () => Promise<unknown>): Promise<Error> {
  try {
    await call();
  } catch (err) {
    return err as Error;
  }
  throw new Error("resolved instead of refusing");
}

describe("createTestContext", () => {
  it("reads the whole oxy-app.json plus a function name, and names a missing function", () => {
    expect(canary().gates).toMatchObject({
      slug: "platform-canary",
      writerSchema: "app_platform_canary",
      oltp: true,
      airhouse: false,
      destinations: ["canary_warehouse"]
    });
    expect(() => createTestContext(manifest, { function: "ghost" })).toThrow(
      /declares no function "ghost"/
    );
  });

  it("is typed as the SDK's OxyFunctionContext", () => {
    const ctx: OxyFunctionContext = canary().ctx;
    expect(typeof ctx.crypto.hmac).toBe("function");
    expect(ctx.airhouse.schema).toBeNull();
  });
});

describe("the manifest's capability gates, in the host's words", () => {
  it("refuses secrets.set with the host's doubled surface", async () => {
    const t = echo();
    const err = await refusal(() => t.ctx.secrets.set("k", "v"));
    expect(err.name).toBe(HOST_ERROR_NAME);
    expect(err.message).toBe(`ctx.secrets.set: ${REFUSALS.secretsWrite.refusal}`);
    expect(t.calls).toEqual([
      {
        op: "secrets.set",
        args: { key: "k", value: "v" },
        outcome: "refused",
        message: err.message
      }
    ]);
  });

  it("refuses email, org, storage, oltp and airhouse by name", async () => {
    const t = echo();
    expect(
      (await refusal(() => t.ctx.email.send({ to: "a@b.c", subject: "s", text: "t" }))).message
    ).toBe(`ctx.email.send: ${REFUSALS.emailSend.refusal}`);
    expect((await refusal(() => t.ctx.org.people())).message).toBe(
      `ctx.org.people: ${REFUSALS.orgPeople.refusal}`
    );
    expect((await refusal(() => t.ctx.org.places())).message).toBe(
      `ctx.org.places: ${REFUSALS.orgMember.refusal.replace("{member}", "places")}`
    );
    expect((await refusal(() => t.ctx.storage.put("a.txt", "x"))).message).toBe(
      `ctx.storage: ${REFUSALS.storageWrite.refusal}`
    );
    expect((await refusal(() => t.ctx.storage.list())).message).toBe(
      `ctx.storage: ${REFUSALS.storageRead.refusal}`
    );
    expect((await refusal(() => t.ctx.oltp.query("SELECT 1 AS one"))).message).toBe(
      `ctx.oltp: ${REFUSALS.oltp.refusal}`
    );
    expect((await refusal(() => t.ctx.oltp.tx(async () => 1))).message).toBe(
      `ctx.tx: ${REFUSALS.oltp.refusal}`
    );
    expect((await refusal(() => t.ctx.airhouse.query("SELECT 1 AS one"))).message).toBe(
      `ctx.airhouse: ${REFUSALS.airhouse.refusal}`
    );
    expect(t.ops()).toEqual<HostOp[]>([
      "email.send",
      "org.people",
      "org.places",
      "storage.put",
      "storage.list",
      "oltp.query",
      "tx.begin_oltp",
      "airhouse.query"
    ]);
    expect(t.calls.every((c) => c.outcome === "refused")).toBe(true);
  });

  it("copy needs both storage gates", async () => {
    const t = createTestContext(
      { slug: "s", functions: { f: { storage: { read: true } } } },
      { function: "f" }
    );
    expect((await refusal(() => t.ctx.storage.copy("a", "b"))).message).toBe(
      `ctx.storage: ${REFUSALS.storageWrite.refusal}`
    );
  });
});

describe("writes to a named database, in the host's order", () => {
  it("refuses a database outside destinations, then one not configured, then by kind", async () => {
    const t = canary();
    const outside = await refusal(() => t.ctx.warehouse.insert("appdb", "t", [{ a: 1 }]));
    expect(outside.message).toBe(
      `ctx.warehouse: ${REFUSALS.destinationNotDeclared.refusal.replace("{database}", "appdb")}`
    );

    const declared = createTestContext(
      {
        slug: "s",
        functions: { f: { destinations: ["missing", "org_oltp", "facts", "canary_warehouse"] } }
      },
      { function: "f", databases: DATABASES }
    );
    expect(
      (await refusal(() => declared.ctx.warehouse.exec("missing", "DELETE FROM t"))).message
    ).toBe(
      `ctx.warehouse: ${REFUSALS.databaseNotConfigured.refusal.replace("{database}", "missing")}`
    );
    expect(
      (await refusal(() => declared.ctx.warehouse.exec("org_oltp", "DELETE FROM t"))).message
    ).toBe(`ctx.warehouse: ${REFUSALS.managedOltpWrite.refusal.replace("{database}", "org_oltp")}`);
    const customer = await refusal(() =>
      declared.ctx.warehouse.insert("canary_warehouse", "t", [{ a: 1 }])
    );
    expect(customer.message).toBe(
      `ctx.warehouse: ${REFUSALS.customerWarehouseWrite.refusal.replace(/\{database\}/g, "canary_warehouse")}`
    );
    const tx = await refusal(() => declared.ctx.tx("canary_warehouse", async () => 1));
    expect(tx.message).toBe(
      `ctx.tx: ${REFUSALS.customerWarehouseWrite.refusal.replace(/\{database\}/g, "canary_warehouse")}${REFUSALS.customerWarehouseTransaction.refusal}`
    );
    await declared.ctx.warehouse.insert("facts", "t", [{ a: 1 }]);
    expect(declared.state.warehouse("facts").rows("t")).toEqual([{ a: 1 }]);
  });

  it("lets a customer-warehouse write through when the function says why", async () => {
    const t = canary();
    await t.ctx.warehouse.exec(
      "canary_warehouse",
      "CREATE TABLE IF NOT EXISTS oxy_canary_writes (run String, path String)"
    );
    await t.ctx.warehouse.insert("canary_warehouse", "oxy_canary_writes", [
      { run: "r", path: "insert-1" }
    ]);
    await t.ctx.warehouse.exec(
      "canary_warehouse",
      "-- a comment first\nINSERT INTO oxy_canary_writes (run, path) VALUES ('r', 'exec-1'), ('r', 'exec-2')"
    );
    const { rows, truncated } = await t.ctx.warehouse.query(
      "canary_warehouse",
      "SELECT path FROM oxy_canary_writes WHERE run = 'r' ORDER BY path"
    );
    expect(rows.map((r) => r.path)).toEqual(["exec-1", "exec-2", "insert-1"]);
    expect(truncated).toBe(false);
    expect(t.ops()).toEqual<HostOp[]>(["warehouse.exec", "warehouse.insert", "warehouse.query"]);
    expect(t.callsTo("warehouse.query")[0]).toMatchObject({ outcome: "ok", source: "author" });
  });
});

describe("upsert_refusal, the spec's worked example", () => {
  it("asks for one upsert, is refused naming ClickHouse, and writes nothing", async () => {
    const t = canary();
    const err = await refusal(() =>
      t.ctx.warehouse.upsert(
        "canary_warehouse",
        "oxy_canary_writes",
        [{ run: "r", path: "upsert-1" }],
        ["run"]
      )
    );
    expect(t.ops()).toEqual<HostOp[]>(["warehouse.upsert"]);
    const [call] = t.calls;
    expect(call.outcome).toBe("refused");
    expect(call.message).toMatch(
      /^ctx\.warehouse: warehouse\.upsert is not supported on ClickHouse/
    );
    expect(err.message).toBe(
      `ctx.warehouse: ${REFUSALS.upsertUnsupported.refusal.replace("{dialect}", "ClickHouse")}`
    );
    expect(call.args).toMatchObject({ database: "canary_warehouse", conflictColumns: ["run"] });
    expect(t.state.warehouse("canary_warehouse").rows("oxy_canary_writes")).toHaveLength(0);
  });

  it("resolves when the host op is overridden, and still records the call", async () => {
    const t = canary();
    t.override("warehouse.upsert", async () => ({}));
    await t.ctx.warehouse.upsert("canary_warehouse", "oxy_canary_writes", [{ run: "r" }], ["run"]);
    expect(t.calls).toEqual([
      expect.objectContaining({ op: "warehouse.upsert", outcome: "ok", result: {} })
    ]);
  });

  it("refuses an Airhouse upsert in the context's own words, as an engine refusal", async () => {
    const t = createTestContext(
      { slug: "s", functions: { f: { destinations: ["facts"] } } },
      { function: "f", databases: DATABASES }
    );
    const err = await refusal(() => t.ctx.warehouse.upsert("facts", "t", [{ id: 1 }], ["id"]));
    expect(err.name).toBe("TestContextError");
    expect(err.message).toMatch(/refused by the ENGINE, not the host/);
    expect(t.calls[0].outcome).toBe("threw");
  });
});

describe("ctx.tx, the runtime's bracket", () => {
  it("refuses on ClickHouse before the callback runs, in the connector's words", async () => {
    const t = canary();
    let ran = false;
    const err = await refusal(() =>
      t.ctx.tx("canary_warehouse", async () => {
        ran = true;
      })
    );
    expect(ran).toBe(false);
    expect(err.message).toBe(
      "ctx.tx: could not open a transaction on 'canary_warehouse': ClickHouse does not support multi-statement transactions — ctx.tx() requires a Postgres-backed database (`type: postgres`). Use ctx.warehouse.{insert,exec,upsert} for single-statement writes."
    );
    expect(t.ops()).toEqual<HostOp[]>(["tx.begin"]);
  });

  it("commits, rolls back on a throw, and refuses a stale handle", async () => {
    const t = canary();
    t.state.oltp.table("canary_roundtrip", { tag: "text", n: "int4" });
    const n = await t.ctx.oltp.tx(async (tx) => {
      expect(await tx.exec("INSERT INTO canary_roundtrip (tag, n) VALUES ($1, $2)", ["k", 7])).toBe(
        1
      );
      const rows = await tx.query("SELECT n FROM canary_roundtrip WHERE tag = $1", ["k"]);
      return rows[0].n;
    });
    expect(n).toBe(7);
    let stale: { query(sql: string): Promise<unknown> } | null = null;
    await expect(
      t.ctx.oltp.tx(async (tx) => {
        stale = tx;
        await tx.exec("DELETE FROM canary_roundtrip WHERE tag = $1", ["k"]);
        throw new Error("author's error");
      })
    ).rejects.toThrow("author's error");
    expect(await t.ctx.oltp.query("SELECT n FROM canary_roundtrip")).toEqual([{ n: 7 }]);
    await expect(
      (stale as unknown as { query(sql: string): Promise<unknown> }).query("SELECT 1")
    ).rejects.toThrow(
      "ctx.oltp.tx: this transaction is already finished — query was called after the callback returned"
    );
    expect(t.ops()).toEqual<HostOp[]>([
      "tx.begin_oltp",
      "tx.exec",
      "tx.query",
      "tx.commit",
      "tx.rollback",
      "oltp.query"
    ]);
    await expect(t.ctx.tx("appdb", "not a function" as never)).rejects.toThrow(TypeError);
  });

  it("caps open transactions at the host's MAX_OPEN", async () => {
    const t = canary();
    const gates: Promise<unknown>[] = [];
    const release: (() => void)[] = [];
    for (let i = 0; i < 4; i++) {
      gates.push(t.ctx.oltp.tx(() => new Promise<void>((resolve) => release.push(resolve))));
    }
    await new Promise((r) => setTimeout(r, 0));
    expect((await refusal(() => t.ctx.oltp.tx(async () => 1))).message).toBe(
      `ctx.tx: ${REFUSALS.transactionsOpen.refusal.replace("{MAX_OPEN}", "4")}`
    );
    for (const r of release) r();
    await Promise.all(gates);
  });
});

describe("rows from the zoo", () => {
  it("answers a zoo read on each plane with the pinned value, and refuses the OLTP numeric case", async () => {
    const t = canary();
    t.state.warehouse("canary_warehouse").zoo();
    t.state.oltp.zoo();
    const wide = ZOO.engines.clickhouse.cases.findIndex(
      (c) => c.key === "clickhouse/UInt64/precision_gt_18"
    );
    const { rows } = await t.ctx.warehouse.query(
      "canary_warehouse",
      `SELECT ${zooColumn(wide)} FROM ${zooTableName()} LIMIT 1`
    );
    expect(rows).toEqual([{ [zooColumn(wide)]: "18446744073709551615" }]);
    expect(t.callsTo("warehouse.query")[0].source).toBe("zoo");

    const numeric = ZOO.engines.postgres.cases.findIndex((c) => c.key === "postgres/numeric/plain");
    const err = await refusal(() =>
      t.ctx.oltp.query(`SELECT ${zooColumn(numeric)} FROM ${zooTableName()} LIMIT 1`)
    );
    expect(err.name).toBe(HOST_ERROR_NAME);
    expect(err.message).toMatch(
      /^ctx\.oltp: result column `c\d+` has Postgres type `numeric`, which cannot be returned directly/
    );
  });

  it("serves ctx.query and queryStream from the default database", async () => {
    const t = canary();
    const result = await t.ctx.query("SELECT 1 AS one");
    expect(result).toEqual({ rows: [{ one: 1 }], truncated: false });
    const batches: unknown[] = [];
    for await (const batch of t.ctx.queryStream("SELECT 1 AS one")) batches.push(batch);
    expect(batches).toEqual([[{ one: 1 }]]);
    expect(t.ops()).toEqual<HostOp[]>(["query", "query_stream"]);
    const none = createTestContext(manifest, { function: "echo" });
    expect((await refusal(() => none.ctx.query("SELECT 1 AS one"))).message).toBe(
      `ctx.query: ${REFUSALS.noDefaultDatabase.refusal}`
    );
  });
});

describe("ctx.fetch", () => {
  it("applies the host's URL rules and byte cap, then answers from the registered reply", async () => {
    const t = canary();
    expect((await refusal(() => t.ctx.fetch("http://example.com/"))).message).toBe(
      `ctx.fetch: ${REFUSALS.fetchBlocked.refusal.replace("{url}", "http://example.com/")}`
    );
    expect((await refusal(() => t.ctx.fetch("https://169.254.169.254/latest"))).message).toMatch(
      /blocked by SSRF allowlist$/
    );
    expect((await refusal(() => t.ctx.fetch("not a url"))).message).toMatch(
      /^ctx\.fetch: invalid url: /
    );
    expect(
      (
        await refusal(() =>
          t.ctx.fetch("https://api.example.test/x", { headers: { "bad header": "v" } })
        )
      ).message
    ).toBe(`ctx.fetch: ${REFUSALS.fetchInvalidHeaderName.refusal.replace("{k}", "bad header")}`);
    expect((await refusal(() => t.ctx.fetch("https://api.example.test/x"))).name).toBe(
      "TestContextError"
    );

    t.state.fetch.on("https://api.example.test/x", { status: 201, body: "ok" });
    t.state.fetch.on(/big/, { body: "x".repeat(11 * 1024 * 1024) });
    expect(await t.ctx.fetch("https://api.example.test/x", { method: "POST", body: "{}" })).toEqual(
      {
        status: 201,
        body: "ok",
        encoding: "utf8"
      }
    );
    expect((await refusal(() => t.ctx.fetch("https://api.example.test/big"))).message).toMatch(
      /^ctx\.fetch: response too large \(\d+ bytes > 10485760 cap\)$/
    );
    // The unanswered call above passed the host's rules, so it was received too.
    expect(t.state.fetch.received.map((r) => r.url)).toEqual([
      "https://api.example.test/x",
      "https://api.example.test/x",
      "https://api.example.test/big"
    ]);
  });
});

describe("storage, secrets, email, org and the rest", () => {
  it("round-trips an object and pages a list with an offset cursor", async () => {
    const t = canary();
    const { key } = await t.ctx.storage.put("canary/a.txt", "hello");
    await t.ctx.storage.put("canary/b.bin", "AQID", {
      encoding: "base64",
      contentType: "application/octet-stream"
    });
    expect(await t.ctx.storage.get(key)).toMatchObject({ body: "hello", encoding: "utf8" });
    expect((await t.ctx.storage.get("canary/b.bin", { encoding: "base64" }))?.body).toBe("AQID");
    expect(await t.ctx.storage.head("missing")).toBeNull();
    const page = await t.ctx.storage.list({ prefix: "canary/", limit: 1 });
    expect(page).toMatchObject({ objects: [{ key: "canary/a.txt" }], hasMore: true });
    const next = await t.ctx.storage.list({
      prefix: "canary/",
      limit: 1,
      cursor: page.cursor ?? undefined
    });
    expect(next).toMatchObject({
      objects: [{ key: "canary/b.bin" }],
      hasMore: false,
      cursor: null
    });
    expect(await t.ctx.storage.delete([key, "canary/b.bin"])).toEqual({ deleted: 2 });
    expect(t.state.storage.size).toBe(0);
  });

  it("records secrets and emails, answers org fixtures, and needs a semantic answer", async () => {
    const t = createTestContext(
      {
        slug: "s",
        functions: { f: { secrets: { write: true }, email: { send: true }, org: { read: true } } }
      },
      { function: "f" }
    );
    await t.ctx.secrets.set("TOKEN", "v");
    expect(t.state.secrets.get("TOKEN")).toBe("v");
    expect(await t.ctx.email.send({ to: "a@b.c", subject: "s", html: "<p>x</p>" })).toEqual({
      messageId: "message-1"
    });
    expect(t.state.emails).toHaveLength(1);
    t.state.org.people.push({ id: "u1", name: "Ana", role: "Shift lead", kind: "member" });
    expect(await t.ctx.org.people()).toEqual({
      people: [{ id: "u1", name: "Ana", role: "Shift lead", kind: "member" }],
      total: 1
    });
    expect(await t.ctx.org.assignments()).toEqual({ assignments: [], total: 0 });
    expect((await refusal(() => t.ctx.semantic.query({ measures: ["x"] }))).name).toBe(
      "TestContextError"
    );
    t.state.semantic.on((spec) => ({ asked: spec }));
    expect(await t.ctx.semantic.query({ measures: ["x"] })).toEqual({ asked: { measures: ["x"] } });
    expect(await t.ctx.airway.run("pipe")).toEqual({ runId: "run-1" });
  });

  it("signs and verifies with ctx.crypto inside t.run, where Buffer is gone", async () => {
    const t = canary();
    const ok = await t.run(() => {
      const signature = t.ctx.crypto.hmac({ key: "secret", data: "body" });
      return {
        verified: t.ctx.crypto.verifyHmac({ key: "secret", data: "body", signature }),
        forged: t.ctx.crypto.verifyHmac({ key: "secret", data: "body", signature: undefined }),
        equal: t.ctx.crypto.timingSafeEqual("a", "a"),
        empty: t.ctx.crypto.timingSafeEqual("", ""),
        buffer: typeof Buffer
      };
    });
    expect(ok).toEqual({
      verified: true,
      forged: false,
      equal: true,
      empty: false,
      buffer: "undefined"
    });
  });

  it("types calls by the host's closed op list", () => {
    const t = canary();
    for (const op of HOST_OPS) expect(t.callsTo(op)).toEqual([]);
    // @ts-expect-error — not a host op
    expect(() => t.callsTo("warehouse.inserts")).not.toThrow();
  });
});

describe("ctx.fetch encodes the bytes, as the host does", () => {
  // `host.rs` encodes what it received: utf8 is a lossy string of the bytes,
  // base64 is the bytes encoded. Handing the fixture back under a different
  // label — the bug this replaces — corrupts a binary body silently, which is
  // the one ctx.fetch gotcha product-context.md names.
  const HELLO_B64 = "aGVsbG8="; // "hello"

  it("encodes a text fixture when the caller asks for base64", async () => {
    const t = canary();
    t.state.fetch.on("https://example.com/x", { status: 200, body: "hello" });
    const res = await t.run(() => t.ctx.fetch("https://example.com/x", { encoding: "base64" }));
    expect(res).toMatchObject({ body: HELLO_B64, encoding: "base64" });
  });

  it("decodes a base64 fixture when the caller asks for utf8", async () => {
    const t = canary();
    t.state.fetch.on("https://example.com/b", {
      status: 200,
      body: HELLO_B64,
      encoding: "base64"
    });
    const res = await t.run(() => t.ctx.fetch("https://example.com/b"));
    expect(res).toMatchObject({ body: "hello", encoding: "utf8" });
  });

  it("passes a fixture through when the labels already agree", async () => {
    const t = canary();
    t.state.fetch.on("https://example.com/p", { status: 200, body: "plain" });
    const res = await t.run(() => t.ctx.fetch("https://example.com/p"));
    expect(res).toMatchObject({ body: "plain", encoding: "utf8" });
  });

  it("refuses an encoding the host does not know", async () => {
    const t = canary();
    t.state.fetch.on("https://example.com/h", { status: 200, body: "x" });
    const err = await refusal(() =>
      // @ts-expect-error — the host's own arm rejects anything but utf8/base64
      t.run(() => t.ctx.fetch("https://example.com/h", { encoding: "hex" }))
    );
    expect(err.message).toMatch(/unknown encoding 'hex'/);
  });
});

describe("a slug that cannot back a schema is refused, not assumed", () => {
  // `WriterCapability` has three states; gating on the boolean alone gave a
  // green test for an app whose ctx.oltp is closed in production.
  const withSlug = (slug: string) =>
    createTestContext(
      { ...manifest, slug, functions: { ...manifest.functions } },
      { function: "canary", databases: DATABASES }
    );

  it("refuses ctx.oltp in the host's words", async () => {
    const t = withSlug("platform_canary"); // an underscore derives no schema
    const err = await refusal(() => t.run(() => t.ctx.oltp.query("SELECT 1 AS one")));
    expect(err.name).toBe(HOST_ERROR_NAME);
    expect(err.message).toMatch(/cannot back an OLTP schema/);
    expect(err.message).toContain("platform_canary");
  });

  it("refuses a leading digit and an over-long slug, as validate_name does", async () => {
    for (const slug of ["9lives", "a".repeat(57)]) {
      const t = withSlug(slug);
      const err = await refusal(() => t.run(() => t.ctx.oltp.query("SELECT 1 AS one")));
      expect(err.message).toMatch(/cannot back an OLTP schema/);
    }
  });

  it("still serves a slug that does derive one", async () => {
    // Outside `t.run`: the globals it removes include ones vitest's own
    // assertion and async machinery need, so a test asserts around `run`, never
    // inside it. See "Asserting around t.run" in the module docs.
    const t = withSlug("platform-canary");
    await t.ctx.oltp.exec("CREATE TABLE t (a TEXT)");
    // `ctx.oltp.query` resolves the rows themselves, unlike ctx.query.
    const rows = await t.ctx.oltp.query("SELECT count(*) FROM t");
    expect(rows).toHaveLength(1);
  });
});

describe("counting, sizing and bracketing", () => {
  it("reports a stored object's size without the base64 padding", async () => {
    const t = canary();
    // `put` base64-encodes a utf8 body before storing, so 5 bytes in is
    // "aGVsbG8=" stored — 8 chars with one pad byte, 5 bytes of content.
    const put = await t.ctx.storage.put("k", "hello", { contentType: "text/plain" });
    expect(put.size).toBe(5);
  });

  it("an overridden tx.begin does not desync the open counter", async () => {
    // `open++` lives inside the begin impl an override replaces, while `open--`
    // ran unconditionally — two overridden transactions left the counter at -2
    // and the MAX_OPEN refusal never fired again. Counting the calls does not
    // show that: the counter is private, so the only way to pin it is to spend
    // the slack. Two overridden OLTP transactions, then MAX_OPEN real warehouse
    // ones (one shared counter), and the next must still be refused — on the
    // unfixed code the counter is at 2, and it is not.
    const t = canary();
    t.override("tx.begin", () => ({ id: 1 }));
    await t.ctx.tx("appdb", async () => "one");
    await t.ctx.tx("appdb", async () => "two");
    expect(t.callsTo("tx.begin")).toHaveLength(2);

    const gates: Promise<unknown>[] = [];
    const release: (() => void)[] = [];
    for (let i = 0; i < 4; i++) {
      gates.push(t.ctx.oltp.tx(() => new Promise<void>((resolve) => release.push(resolve))));
    }
    await new Promise((r) => setTimeout(r, 0));
    expect((await refusal(() => t.ctx.oltp.tx(async () => 1))).message).toBe(
      `ctx.tx: ${REFUSALS.transactionsOpen.refusal.replace("{MAX_OPEN}", "4")}`
    );
    for (const r of release) r();
    await Promise.all(gates);
  });
});
