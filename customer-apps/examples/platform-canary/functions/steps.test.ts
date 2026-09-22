// The canary's step logic, against a fake `ctx`.
//
// The fake is a small, well-behaved host: warehouse writes land in an in-memory
// table, OLTP rows and storage objects live in maps, and every member records
// its call. A test breaks one member and checks that the failure surfaces as
// `canary step <name> failed: …` — the prefix the pager fingerprints on.
//
// The fake is also a ClickHouse: `warehouse.upsert` and `ctx.tx` refuse in the
// host's words, which two steps pin.

import type { OxyFunctionContext } from "@oxy-hq/sdk";
import { base64ToBytes, bytesToBase64 } from "@oxy-hq/sdk";
import { build } from "esbuild";
import type { ShapeZoo, ZooCase } from "./shape-zoo";
import zooJson from "./shape-zoo.json";
import { describe, expect, it } from "vitest";
import {
  ALL_STEPS,
  CANARY_SECRET_KEY,
  STEP_OPS,
  runCanary,
  type StepName,
  selectSteps
} from "./steps";

const RUN_ID = "run-k3x9a1";
const CHECKIN_URL = "https://allquiet.example.test/api/webhook/checkin";
const UPLOAD_HOST = "https://canary-bucket.s3.example.test/";
const OCTET = "application/octet-stream";

/** As `reply_json` in `runtime.rs` prefixes the host's refusals. */
const UPSERT_REFUSAL =
  "ctx.warehouse: warehouse.upsert is not supported on ClickHouse: it compiles to `INSERT … ON CONFLICT … DO UPDATE`, which only Postgres and DuckDB parse. Use warehouse.insert, or warehouse.exec with this warehouse's own upsert statement.";
const TX_REFUSAL =
  "ctx.tx: could not open a transaction on 'canary_warehouse': ClickHouse does not support multi-statement transactions — ctx.tx() requires a Postgres-backed database (`type: postgres`). Use ctx.warehouse.{insert,exec,upsert} for single-statement writes.";

type Row = Record<string, unknown>;

interface Call {
  member: string;
  args: unknown[];
}

interface TxHandle {
  query(sql: string, params?: unknown[]): Promise<Row[]>;
  exec(sql: string, params?: unknown[]): Promise<number>;
}

type TxFn = (tx: TxHandle) => Promise<unknown> | unknown;

/** One `('run', 'path')` tuple in a VALUES list. */
const TUPLE = /\(\s*'([^']*)'\s*,\s*'([^']*)'\s*\)/g;

const ZOO = zooJson as unknown as ShapeZoo;

/**
 * A well-behaved host's answer to a shape-zoo read. The table already has its row, and every
 * column is its case's expectation; a `$error` case throws that text. `corrupt` swaps every value.
 */
function zooRows(sql: string, cases: ZooCase[], plane: "warehouse" | "oltp", corrupt: boolean): Row[] {
  if (/^SELECT count\(\*\)/.test(sql)) return [{ n: 1 }];
  const match = /^SELECT (c(\d+)) FROM/.exec(sql);
  if (!match) return [];
  const expected = cases[Number(match[2]) - 1].expect[plane];
  if (typeof expected === "object" && expected !== null && "$error" in expected) {
    throw new Error(`refused: ${String((expected as { $error: unknown }).$error)}`);
  }
  return [{ [match[1]]: corrupt ? "corrupted" : expected }];
}

const PLACE = {
  id: "place-1",
  org_id: "org-1",
  name: "Clovis",
  kind: "store",
  parent_id: null,
  status: "open",
  timezone: "America/Los_Angeles",
  external_id: null,
  external_ids: { toast: "toast-1" },
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z"
};

const PERSON = { id: "user-1", name: "Ana", role: "Shift lead", kind: "member" };

const ASSIGNMENT = {
  id: "assignment-1",
  user_id: "user-1",
  user_name: "Ana",
  user_kind: "member",
  role_id: "role-1",
  role_name: "Shift lead",
  role_scope: "location",
  location_id: "place-1",
  location_name: "Clovis",
  supervisor_id: null,
  supervisor_name: null,
  created_at: "2026-01-01T00:00:00Z"
};

/** A host that decoded the object as UTF-8 would mangle its high bytes; flipping the last byte stands in for that. */
function flipLastByte(base64: string): string {
  const bytes = base64ToBytes(base64);
  bytes[bytes.length - 1] ^= 0xff;
  return bytesToBase64(bytes);
}

function makeFake() {
  const calls: Call[] = [];
  const record = (member: string, ...args: unknown[]) => {
    calls.push({ member, args });
  };
  const table: Row[] = [];
  // OLTP rows by run tag. The fake reads the tag from the first bound parameter.
  const oltpRows = new Set<string>();
  // Storage objects: key → base64 body.
  const objects = new Map<string, string>();
  const state = {
    dropReadbackRows: 0,
    corruptGet: false,
    corruptZoo: false,
    corruptDownload: false,
    hideFromList: false,
    dropCommit: false,
    leakRollback: false,
    resolveOnThrow: false
  };

  const oltpWrite = (rows: Set<string>, sql: string, params: unknown[]): Row[] => {
    const tags = params.map(String);
    if (/^\s*INSERT/i.test(sql)) {
      for (const tag of tags) rows.add(tag);
      return tags.map((run) => ({ run }));
    }
    if (/^\s*DELETE/i.test(sql)) {
      return tags.filter((tag) => rows.delete(tag)).map((run) => ({ run }));
    }
    return [];
  };

  const oltpRead = (visible: (tag: string) => boolean, sql: string, params: unknown[]): Row[] => {
    if (/^\s*SELECT/i.test(sql)) {
      const run = String(params[0]);
      return visible(run) ? [{ run }] : [];
    }
    return [];
  };

  const sizeOf = (body: string, encoding?: string) =>
    encoding === "base64" ? base64ToBytes(body).length : body.length;

  const raw = {
    env: {
      [CANARY_SECRET_KEY]: `run-previous@${Date.now() - 5 * 60_000}`
    } as Record<string, string>,
    log: (..._args: unknown[]) => {},
    query: async (sql: string): Promise<unknown> => {
      record("query", sql);
      return { rows: [{ one: 1 }], truncated: false };
    },
    queryStream: async function* (sql: string): AsyncGenerator<Row[], void, unknown> {
      record("query_stream", sql);
      yield [{ one: 1 }];
    },
    warehouse: {
      insert: async (database: string, tableName: string, rows: Row[]): Promise<unknown> => {
        record("warehouse.insert", database, tableName, rows);
        table.push(...rows);
        return {};
      },
      exec: async (database: string, sql: string): Promise<unknown> => {
        record("warehouse.exec", database, sql);
        if (sql.includes("oxy_shape_zoo_")) return {};
        if (/\bINSERT\b/i.test(sql)) {
          for (const [, run, path] of sql.matchAll(TUPLE)) table.push({ run, path });
        }
        return {};
      },
      query: async (database: string, sql: string): Promise<unknown> => {
        record("warehouse.query", database, sql);
        if (sql.includes("oxy_shape_zoo_")) {
          const rows = zooRows(sql, ZOO.engines.clickhouse.cases, "warehouse", state.corruptZoo);
          return { rows, truncated: false };
        }
        const landed = table.filter((row) => sql.includes(`'${String(row.run)}'`));
        return { rows: landed.slice(0, landed.length - state.dropReadbackRows), truncated: false };
      },
      // The fake is a ClickHouse: no ON CONFLICT, so the host refuses by name.
      upsert: async (
        database: string,
        tableName: string,
        rows: Row[],
        conflictColumns: string[]
      ): Promise<unknown> => {
        record("warehouse.upsert", database, tableName, rows, conflictColumns);
        throw new Error(UPSERT_REFUSAL);
      }
    },
    // …and no transactions either. `fn` never runs.
    tx: async (database: string, _fn: TxFn): Promise<unknown> => {
      record("tx.begin", database);
      throw new Error(TX_REFUSAL);
    },
    oltp: {
      exec: async (sql: string, params: unknown[] = []): Promise<number> => {
        record("oltp.exec", sql, params);
        if (sql.includes("oxy_shape_zoo_")) return 0;
        return oltpWrite(oltpRows, sql, params).length;
      },
      query: async (sql: string, params: unknown[] = []): Promise<Row[]> => {
        record("oltp.query", sql, params);
        if (sql.includes("oxy_shape_zoo_")) {
          return zooRows(sql, ZOO.engines.postgres.cases, "oltp", state.corruptZoo);
        }
        return oltpRead((tag) => oltpRows.has(tag), sql, params);
      },
      // The commit/rollback bracket of `ctx.oltp.tx`: writes stage on the handle
      // and reach `oltpRows` on commit; a throw rolls them back and rethrows.
      tx: async (fn: TxFn): Promise<unknown> => {
        record("tx.begin_oltp");
        const staged = new Set<string>();
        const handle: TxHandle = {
          query: async (sql, params = []) => {
            record("tx.query", sql, params);
            return oltpRead((tag) => staged.has(tag) || oltpRows.has(tag), sql, params);
          },
          exec: async (sql, params = []) => {
            record("tx.exec", sql, params);
            return oltpWrite(staged, sql, params).length;
          }
        };
        let result: unknown;
        try {
          result = await fn(handle);
        } catch (err) {
          record("tx.rollback");
          if (state.leakRollback) for (const tag of staged) oltpRows.add(tag);
          if (state.resolveOnThrow) return undefined;
          throw err;
        }
        record("tx.commit");
        if (!state.dropCommit) for (const tag of staged) oltpRows.add(tag);
        return result;
      }
    },
    org: {
      places: async (): Promise<unknown> => {
        record("org.places");
        return { places: [PLACE], total: 1 };
      },
      people: async (): Promise<unknown> => {
        record("org.people");
        return { people: [PERSON], total: 1 };
      },
      assignments: async (): Promise<unknown> => {
        record("org.assignments");
        return { assignments: [ASSIGNMENT], total: 1 };
      }
    },
    storage: {
      put: async (pathname: string, body: string, opts?: { encoding?: string }) => {
        record("storage.put", pathname, body, opts);
        objects.set(pathname, body);
        return { key: pathname, size: sizeOf(body, opts?.encoding), contentType: OCTET };
      },
      get: async (key: string, opts?: { encoding?: string }) => {
        record("storage.get", key, opts);
        const body = objects.get(key);
        if (body === undefined) return null;
        const served = state.corruptGet ? flipLastByte(body) : body;
        return {
          body: served,
          contentType: OCTET,
          size: base64ToBytes(served).length,
          encoding: "base64"
        };
      },
      getUploadUrl: async (input: { pathname?: string; contentLength: number }) => {
        record("storage.getUploadUrl", input);
        const key = input.pathname ?? "uploads/unnamed";
        return {
          url: `${UPLOAD_HOST}${key}?X-Amz-Signature=fake`,
          key,
          expiresAt: new Date(Date.now() + 900_000).toISOString()
        };
      },
      getDownloadUrl: async (key: string, opts?: { expiresInSeconds?: number }) => {
        record("storage.getDownloadUrl", key, opts);
        return {
          url: `${UPLOAD_HOST}${key}?X-Amz-Signature=fake&response-content-disposition=inline`,
          expiresAt: new Date(Date.now() + 900_000).toISOString()
        };
      },
      head: async (key: string) => {
        record("storage.head", key);
        const body = objects.get(key);
        return body === undefined
          ? null
          : { key, size: base64ToBytes(body).length, contentType: OCTET };
      },
      // Sorted keys and an offset cursor, as `local::list` pages; S3 pages the
      // same order by continuation token.
      list: async (opts?: { prefix?: string; limit?: number; cursor?: string }) => {
        record("storage.list", opts);
        const prefix = opts?.prefix ?? "";
        const limit = opts?.limit ?? 100;
        const offset = Number(opts?.cursor ?? 0);
        const all = [...objects.entries()]
          .filter(([key]) => key.startsWith(prefix))
          .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
          .map(([key, body]) => ({ key, size: base64ToBytes(body).length, contentType: OCTET }));
        if (state.hideFromList) {
          // Hide the run's own put; a listing without it is the fake's bug, not a hidden object.
          const at = all.findIndex((object) => object.key === `canary/${RUN_ID}.bin`);
          if (at < 0) throw new Error("hideFromList: the run's put key is not in the listing");
          all.splice(at, 1);
        }
        const page = all.slice(offset, offset + limit);
        const end = offset + page.length;
        const hasMore = end < all.length;
        return { objects: page, cursor: hasMore ? String(end) : null, hasMore };
      },
      delete: async (keyOrKeys: string | string[]) => {
        record("storage.delete", keyOrKeys);
        const keys = Array.isArray(keyOrKeys) ? keyOrKeys : [keyOrKeys];
        for (const key of keys) objects.delete(key);
        return { deleted: keys.length };
      },
      copy: async (fromKey: string, toPathname: string, opts?: { allowOverwrite?: boolean }) => {
        record("storage.copy", fromKey, toPathname, opts);
        const body = objects.get(fromKey);
        if (body === undefined) throw new Error(`storage.copy: source '${fromKey}' not found`);
        objects.set(toPathname, body);
        return { key: toPathname, size: base64ToBytes(body).length, contentType: OCTET };
      }
    },
    secrets: {
      set: async (key: string, value: string): Promise<void> => {
        record("secrets.set", key, value);
      }
    },
    // A PUT to an upload URL stores the body; a GET of a download URL serves it
    // as base64 when asked to, the way the host decodes a binary response.
    fetch: async (
      url: string,
      init?: { method?: string; body?: unknown; bodyEncoding?: string; encoding?: string }
    ): Promise<{ status: number; body: string; encoding: string }> => {
      record("fetch", url, init);
      if (url.startsWith(UPLOAD_HOST)) {
        const key = url.slice(UPLOAD_HOST.length).split("?")[0];
        if (init?.method === "PUT") {
          objects.set(key, String(init.body));
        } else if ((init?.method ?? "GET") === "GET") {
          const body = objects.get(key);
          if (body === undefined) return { status: 404, body: "", encoding: "utf8" };
          const served = state.corruptDownload ? flipLastByte(body) : body;
          return { status: 200, body: served, encoding: init?.encoding ?? "utf8" };
        }
      }
      return { status: 200, body: "", encoding: "utf8" };
    }
  };

  return {
    raw,
    ctx: raw as unknown as OxyFunctionContext,
    calls,
    table,
    oltpRows,
    objects,
    state,
    record
  };
}

type Fake = ReturnType<typeof makeFake>;

/** `checkinUrl: null` runs without one (an `undefined` argument would take the default). */
function run(fake: Fake, steps: StepName[] = ALL_STEPS, checkinUrl: string | null = CHECKIN_URL) {
  return runCanary(fake.ctx, { runId: RUN_ID, steps, checkinUrl: checkinUrl ?? undefined });
}

function callsTo(fake: Fake, member: string): Call[] {
  return fake.calls.filter((call) => call.member === member);
}

async function failure(promise: Promise<unknown>): Promise<Error> {
  try {
    await promise;
  } catch (err) {
    return err as Error;
  }
  throw new Error("expected the canary to fail");
}

describe("runCanary", () => {
  it("runs every step against a well-behaved host", async () => {
    const fake = makeFake();
    await expect(run(fake)).resolves.toEqual({ ok: true, runId: RUN_ID, steps: ALL_STEPS });
  });

  it("refuses a run id that cannot be inlined into SQL, before any call", async () => {
    const fake = makeFake();
    await expect(
      runCanary(fake.ctx, {
        runId: "x'); DROP TABLE t; --",
        steps: ALL_STEPS,
        checkinUrl: CHECKIN_URL
      })
    ).rejects.toThrow(/run id/);
    expect(fake.calls).toHaveLength(0);
  });

  describe("warehouse", () => {
    it("warehouse_insert sends three rows in one insert, each tagged with the run id", async () => {
      const fake = makeFake();
      await run(fake, ["warehouse_insert"]);
      const inserts = callsTo(fake, "warehouse.insert");
      expect(inserts).toHaveLength(1);
      const [database, , rows] = inserts[0].args as [string, string, Row[]];
      expect(database).toBe("canary_warehouse");
      expect(rows).toHaveLength(3);
      expect(rows.every((row) => row.run === RUN_ID)).toBe(true);
    });

    it("warehouse_exec sends one multi-row INSERT that opens with a comment", async () => {
      const fake = makeFake();
      await run(fake, ["warehouse_exec"]);
      const inserts = callsTo(fake, "warehouse.exec")
        .map((call) => String(call.args[1]))
        .filter((sql) => /\bINSERT\b/i.test(sql));
      expect(inserts).toHaveLength(1);
      expect(inserts[0].startsWith("--")).toBe(true);
      const tuples = [...inserts[0].matchAll(TUPLE)];
      expect(tuples).toHaveLength(2);
      expect(tuples.every(([, runTag]) => runTag === RUN_ID)).toBe(true);
    });

    it("warehouse_readback fails when only 4 of the 5 rows come back", async () => {
      const fake = makeFake();
      fake.state.dropReadbackRows = 1;
      await expect(run(fake)).rejects.toThrow(/^canary step warehouse_readback failed: /);
      expect(fake.table).toHaveLength(5);
    });

    it("a throwing insert names warehouse_insert and keeps the host's cause", async () => {
      const fake = makeFake();
      fake.raw.warehouse.insert = async () => {
        throw new Error(
          "Code: 27. DB::Exception: Cannot parse input: expected '(' before: ' -- oxy'"
        );
      };
      const err = await failure(run(fake));
      expect(err.message.startsWith("canary step warehouse_insert failed: ")).toBe(true);
      expect(err.message).toContain("Code: 27");
    });
  });

  describe("refusals the destination's engine decides", () => {
    it("upsert_refusal asks for one upsert, is refused naming ClickHouse, and writes nothing", async () => {
      const fake = makeFake();
      await run(fake, ["upsert_refusal"]);
      expect(fake.calls.map((call) => call.member)).toEqual(["warehouse.upsert"]);
      const [database, , rows, conflict] = fake.calls[0].args as [string, string, Row[], string[]];
      expect(database).toBe("canary_warehouse");
      expect(rows.every((row) => row.run === RUN_ID)).toBe(true);
      expect(conflict).toEqual(["run"]);
      expect(fake.table).toHaveLength(0);
    });

    it("upsert_refusal fails when the host accepts the upsert", async () => {
      const fake = makeFake();
      fake.raw.warehouse.upsert = async () => ({});
      await expect(run(fake, ["upsert_refusal"])).rejects.toThrow(
        /^canary step upsert_refusal failed: ctx\.warehouse\.upsert resolved on ClickHouse instead of refusing/
      );
    });

    it("upsert_refusal fails when the refusal is in other words, quoting them", async () => {
      const fake = makeFake();
      fake.raw.warehouse.upsert = async () => {
        throw new Error("Code: 62. DB::Exception: Syntax error: failed at position 60 (CONFLICT)");
      };
      const err = await failure(run(fake, ["upsert_refusal"]));
      expect(err.message).toMatch(/^canary step upsert_refusal failed: ctx\.warehouse\.upsert was refused in other words/);
      expect(err.message).toContain("Code: 62");
    });

    it("tx_refusal opens one transaction on the warehouse, is refused naming ClickHouse, and the callback never runs", async () => {
      const fake = makeFake();
      await run(fake, ["tx_refusal"]);
      expect(fake.calls.map((call) => call.member)).toEqual(["tx.begin"]);
      expect(fake.calls[0].args[0]).toBe("canary_warehouse");
    });

    it("tx_refusal fails when the host opens a transaction on ClickHouse", async () => {
      const fake = makeFake();
      fake.raw.tx = async (_database, fn) =>
        fn({ query: async () => [], exec: async () => 0 });
      await expect(run(fake, ["tx_refusal"])).rejects.toThrow(
        /^canary step tx_refusal failed: ctx\.tx resolved on ClickHouse instead of refusing/
      );
    });
  });

  describe("sql_stream", () => {
    it("reads one row through the generator, from one host call", async () => {
      const fake = makeFake();
      await run(fake, ["sql_stream"]);
      expect(callsTo(fake, "query_stream")).toHaveLength(1);
      expect(callsTo(fake, "query")).toHaveLength(0);
    });

    it("fails when the generator yields nothing", async () => {
      const fake = makeFake();
      fake.raw.queryStream = async function* () {};
      await expect(run(fake, ["sql_stream"])).rejects.toThrow(
        /^canary step sql_stream failed: expected one row with one = 1, got 0 rows/
      );
    });
  });

  describe("oltp_roundtrip", () => {
    it("deletes the row it wrote", async () => {
      const fake = makeFake();
      await run(fake, ["oltp_roundtrip"]);
      const oltp = fake.calls.filter((call) => call.member.startsWith("oltp."));
      const touches = (verb: RegExp) => (call: Call) =>
        verb.test(String(call.args[0])) && (call.args[1] as unknown[]).includes(RUN_ID);
      const insertAt = oltp.findIndex(touches(/^\s*INSERT/i));
      const deleteAt = oltp.findIndex(touches(/^\s*DELETE/i));
      expect(insertAt).toBeGreaterThanOrEqual(0);
      expect(deleteAt).toBeGreaterThan(insertAt);
      expect(fake.oltpRows.size).toBe(0);
    });

    it("fails when the delete throws", async () => {
      const fake = makeFake();
      const { exec, query } = fake.raw.oltp;
      const refuseDelete =
        <T>(fn: (sql: string, params?: unknown[]) => Promise<T>) =>
        async (sql: string, params?: unknown[]): Promise<T> => {
          if (/^\s*DELETE/i.test(sql))
            throw new Error("permission denied for table canary_roundtrip");
          return fn(sql, params);
        };
      fake.raw.oltp.exec = refuseDelete(exec);
      fake.raw.oltp.query = refuseDelete(query);
      await expect(run(fake, ["oltp_roundtrip"])).rejects.toThrow(
        /^canary step oltp_roundtrip failed: /
      );
    });
  });

  describe("oltp_transaction", () => {
    it("commits one tagged row, rolls a second back, and deletes what it committed", async () => {
      const fake = makeFake();
      await run(fake, ["oltp_transaction"]);
      const members = fake.calls.map((call) => call.member);
      // One commit, one rollback, in that order, each after its own begin.
      expect(members.filter((m) => m === "tx.begin_oltp")).toHaveLength(2);
      expect(members.indexOf("tx.commit")).toBeLessThan(members.indexOf("tx.rollback"));
      const inserted = callsTo(fake, "tx.exec")
        .filter((call) => /^\s*INSERT/i.test(String(call.args[0])))
        .map((call) => String((call.args[1] as unknown[])[0]));
      expect(inserted).toEqual([`${RUN_ID}-tx`, `${RUN_ID}-rb`]);
      expect(fake.oltpRows.size).toBe(0);
    });

    it("fails when the commit does not land", async () => {
      const fake = makeFake();
      fake.state.dropCommit = true;
      await expect(run(fake, ["oltp_transaction"])).rejects.toThrow(
        /^canary step oltp_transaction failed: after commit: 0 rows tagged run-k3x9a1-tx, expected 1/
      );
    });

    it("fails when the rollback leaves the row, and still deletes it", async () => {
      const fake = makeFake();
      fake.state.leakRollback = true;
      await expect(run(fake, ["oltp_transaction"])).rejects.toThrow(
        /^canary step oltp_transaction failed: after rollback: 1 rows tagged run-k3x9a1-rb, expected 0/
      );
      expect(fake.oltpRows.size).toBe(0);
    });

    it("fails when ctx.oltp.tx resolves although the callback threw", async () => {
      const fake = makeFake();
      fake.state.resolveOnThrow = true;
      await expect(run(fake, ["oltp_transaction"])).rejects.toThrow(
        /^canary step oltp_transaction failed: ctx\.oltp\.tx resolved although the callback threw/
      );
    });
  });

  describe("storage_roundtrip", () => {
    it("puts binary as base64, reads the same bytes back, and deletes the object", async () => {
      const fake = makeFake();
      await run(fake, ["storage_roundtrip"]);
      const [put] = callsTo(fake, "storage.put");
      const [pathname, body, opts] = put.args as [string, string, { encoding?: string }];
      expect(opts?.encoding).toBe("base64");
      expect(pathname).toContain(RUN_ID);
      expect(base64ToBytes(body).some((byte) => byte >= 0x80)).toBe(true);
      const gets = callsTo(fake, "storage.get");
      expect(
        gets.some(
          (call) =>
            call.args[0] === pathname &&
            (call.args[1] as { encoding?: string })?.encoding === "base64"
        )
      ).toBe(true);
      const deleted = callsTo(fake, "storage.delete").flatMap((call) => [call.args[0]].flat());
      expect(deleted).toContain(pathname);
      expect(fake.objects.size).toBe(0);
    });

    it("fails when the bytes come back different", async () => {
      const fake = makeFake();
      fake.state.corruptGet = true;
      await expect(run(fake, ["storage_roundtrip"])).rejects.toThrow(
        /^canary step storage_roundtrip failed: /
      );
    });

    it("PUTs real bytes to a presigned upload URL through ctx.fetch, then heads it", async () => {
      const fake = makeFake();
      await run(fake, ["storage_roundtrip"]);
      const [upload] = callsTo(fake, "storage.getUploadUrl");
      const { pathname } = upload.args[0] as { pathname: string };
      expect(pathname).toContain(RUN_ID);
      const puts = callsTo(fake, "fetch").filter(
        (call) => (call.args[1] as { method?: string })?.method === "PUT"
      );
      expect(puts).toHaveLength(1);
      expect((puts[0].args[1] as { bodyEncoding?: string }).bodyEncoding).toBe("base64");
      expect(callsTo(fake, "storage.head").map((call) => call.args[0])).toContain(pathname);
    });

    it("fails when the presigned PUT is refused, and still deletes what it wrote", async () => {
      const fake = makeFake();
      fake.raw.fetch = async (url: string, init?: { method?: string }) => {
        fake.record("fetch", url, init);
        return { status: 403, body: "SignatureDoesNotMatch", encoding: "utf8" };
      };
      await expect(run(fake, ["storage_roundtrip"])).rejects.toThrow(
        /^canary step storage_roundtrip failed: presigned PUT answered HTTP 403/
      );
      expect(fake.objects.size).toBe(0);
    });

    it("fails when head cannot see the uploaded object", async () => {
      const fake = makeFake();
      fake.raw.fetch = async (url: string, init?: { method?: string }) => {
        fake.record("fetch", url, init);
        return { status: 200, body: "", encoding: "utf8" };
      };
      await expect(run(fake, ["storage_roundtrip"])).rejects.toThrow(
        /^canary step storage_roundtrip failed: head saw no bytes/
      );
    });

    it("copies the object, GETs the copy through its presigned URL as base64, and deletes all three", async () => {
      const fake = makeFake();
      await run(fake, ["storage_roundtrip"]);
      const [copy] = callsTo(fake, "storage.copy");
      const [fromKey, toPathname] = copy.args as [string, string];
      expect(fromKey).toBe(`canary/${RUN_ID}.bin`);
      expect(toPathname).toBe(`canary/${RUN_ID}-copy.bin`);
      const [download] = callsTo(fake, "storage.getDownloadUrl");
      expect(download.args[0]).toBe(toPathname);
      const gets = callsTo(fake, "fetch").filter(
        (call) => (call.args[1] as { method?: string })?.method !== "PUT"
      );
      expect(gets).toHaveLength(1);
      expect((gets[0].args[1] as { encoding?: string }).encoding).toBe("base64");
      const deleted = callsTo(fake, "storage.delete").flatMap((call) => [call.args[0]].flat());
      expect(deleted).toHaveLength(3);
      expect(deleted).toContain(toPathname);
      expect(fake.objects.size).toBe(0);
    });

    it("lists under canary/, a directory boundary, and expects every object it wrote", async () => {
      const fake = makeFake();
      await run(fake, ["storage_roundtrip"]);
      const lists = callsTo(fake, "storage.list");
      expect(lists).toHaveLength(1);
      expect((lists[0].args[0] as { prefix?: string }).prefix).toBe("canary/");
      fake.state.hideFromList = true;
      await expect(run(fake, ["storage_roundtrip"])).rejects.toThrow(
        /^canary step storage_roundtrip failed: list under canary\/ lacks 1 of the 3 objects/
      );
      expect(fake.objects.size).toBe(0);
    });

    it("walks list pages, so objects an earlier run left behind cannot hide this run's", async () => {
      const fake = makeFake();
      // 150 leftovers that sort before this run's keys: on one page of 100, none of the run's would be seen.
      for (let i = 0; i < 150; i++) {
        fake.objects.set(`canary/a-older-${String(i).padStart(3, "0")}.bin`, "AA==");
      }
      await run(fake, ["storage_roundtrip"]);
      const lists = callsTo(fake, "storage.list");
      expect(lists).toHaveLength(2);
      expect((lists[1].args[0] as { cursor?: string }).cursor).toBe("100");
      // The leftovers are not this run's to delete.
      expect(fake.objects.size).toBe(150);
    });

    it("fails, naming the cause, when the listing never ends", async () => {
      const fake = makeFake();
      for (let i = 0; i < 1001; i++) {
        fake.objects.set(`canary/a-older-${String(i).padStart(4, "0")}.bin`, "AA==");
      }
      await expect(run(fake, ["storage_roundtrip"])).rejects.toThrow(
        /^canary step storage_roundtrip failed: more than 1000 objects under canary\/: earlier runs are not cleaning up/
      );
      expect(callsTo(fake, "storage.list")).toHaveLength(10);
    });

    it("fails when the presigned GET returns different bytes", async () => {
      const fake = makeFake();
      fake.state.corruptDownload = true;
      await expect(run(fake, ["storage_roundtrip"])).rejects.toThrow(
        /^canary step storage_roundtrip failed: presigned GET returned different bytes/
      );
      expect(fake.objects.size).toBe(0);
    });
  });

  describe("secrets_roundtrip", () => {
    it("writes a value tagged with the run id", async () => {
      const fake = makeFake();
      await run(fake, ["secrets_roundtrip"]);
      const sets = callsTo(fake, "secrets.set");
      expect(sets).toHaveLength(1);
      expect(sets[0].args[0]).toBe(CANARY_SECRET_KEY);
      expect(String(sets[0].args[1])).toContain(RUN_ID);
    });

    it("fails when ctx.env has no value from a previous run, and still writes one", async () => {
      const fake = makeFake();
      delete fake.raw.env[CANARY_SECRET_KEY];
      await expect(run(fake, ["secrets_roundtrip"])).rejects.toThrow(
        /^canary step secrets_roundtrip failed: ctx\.env has no CANARY_SECRET_ROUNDTRIP/
      );
      expect(callsTo(fake, "secrets.set")).toHaveLength(1);
    });

    it("fails when the previous run's value is stale", async () => {
      const fake = makeFake();
      fake.raw.env[CANARY_SECRET_KEY] = `run-previous@${Date.now() - 2 * 60 * 60_000}`;
      await expect(run(fake, ["secrets_roundtrip"])).rejects.toThrow(
        /^canary step secrets_roundtrip failed: /
      );
    });
  });

  describe("check_in", () => {
    it("fails without a check-in URL and sends nothing", async () => {
      const fake = makeFake();
      await expect(run(fake, ["check_in"], null)).rejects.toThrow(/^canary step check_in failed: /);
      expect(callsTo(fake, "fetch")).toHaveLength(0);
    });

    it("POSTs once, after every other step", async () => {
      const fake = makeFake();
      await run(fake);
      const checkins = fake.calls
        .map((call, index) => ({ call, index }))
        .filter(({ call }) => call.member === "fetch" && call.args[0] === CHECKIN_URL);
      expect(checkins).toHaveLength(1);
      expect((checkins[0].call.args[1] as { method?: string }).method).toBe("POST");
      expect(checkins[0].index).toBe(fake.calls.length - 1);
    });

    it("does not check in when an earlier step fails", async () => {
      const fake = makeFake();
      fake.raw.org.places = async () => {
        throw new Error("OrgCapabilityMissing");
      };
      await expect(run(fake)).rejects.toThrow(/^canary step org_read failed: /);
      expect(
        fake.calls.some((call) => call.member === "fetch" && call.args[0] === CHECKIN_URL)
      ).toBe(false);
    });

    it("fails on a non-2xx answer without echoing the URL", async () => {
      const fake = makeFake();
      fake.raw.fetch = async (url: string, init?: { method?: string }) => {
        fake.record("fetch", url, init);
        return { status: 503, body: "unavailable", encoding: "utf8" };
      };
      const err = await failure(run(fake, ["check_in"]));
      expect(err.message).toMatch(/^canary step check_in failed: /);
      expect(err.message).not.toContain(CHECKIN_URL);
    });
  });

  it("records a cause without any URL in it", async () => {
    const fake = makeFake();
    fake.raw.fetch = async () => {
      throw new Error(`error sending request for url (${CHECKIN_URL}?token=secret)`);
    };
    const err = await failure(run(fake, ["check_in"]));
    expect(err.message).toMatch(/^canary step check_in failed: /);
    expect(err.message).not.toContain("allquiet.example.test");
    expect(err.message).not.toContain("token=secret");
  });

  it.each([
    ["a bare row array", [{ one: 1 }]],
    ["{ rows } without truncated", { rows: [{ one: 1 }] }]
  ] as Array<[string, unknown]>)(
    "sql_read refuses %s, naming the shape",
    async (_label, answer) => {
      const fake = makeFake();
      fake.raw.query = async () => answer;
      await expect(run(fake, ["sql_read"])).rejects.toThrow(
        /^canary step sql_read failed: ctx\.query did not resolve \{ rows, truncated \}/
      );
    }
  );

  it.each([
    ["places that is not a list", { places: "nope", total: 1 }, /did not resolve \{ places/],
    [
      "a place missing its fields",
      { places: [PLACE, { id: "place-2" }], total: 2 },
      /place 1 lacks/
    ]
  ] as Array<[string, unknown, RegExp]>)(
    "org_read refuses %s, naming what is wrong",
    async (_label, answer, cause) => {
      const fake = makeFake();
      fake.raw.org.places = async () => answer;
      const err = await failure(run(fake, ["org_read"]));
      expect(err.message).toMatch(/^canary step org_read failed: /);
      expect(err.message).toMatch(cause);
    }
  );

  it("org_read reads places, people and assignments, and passes on an empty org", async () => {
    const fake = makeFake();
    fake.raw.org.places = async () => {
      fake.record("org.places");
      return { places: [], total: 0 };
    };
    fake.raw.org.people = async () => {
      fake.record("org.people");
      return { people: [], total: 0 };
    };
    fake.raw.org.assignments = async () => {
      fake.record("org.assignments");
      return { assignments: [], total: 0 };
    };
    await run(fake, ["org_read"]);
    expect(fake.calls.map((call) => call.member)).toEqual([
      "org.places",
      "org.people",
      "org.assignments"
    ]);
  });

  // Every step, broken at its own host call, fails under its own name. The
  // optional last element is the step list, for a step that reads what earlier
  // steps wrote.
  const breakers: Array<[string, StepName, (fake: Fake) => void, StepName[]?]> = [
    [
      "warehouse_exec: exec throws",
      "warehouse_exec",
      (fake) => {
        fake.raw.warehouse.exec = async () => {
          throw new Error("Code: 62. Syntax error");
        };
      }
    ],
    [
      "warehouse_readback: query throws",
      "warehouse_readback",
      (fake) => {
        fake.raw.warehouse.query = async () => {
          throw new Error("Code: 60. Unknown table");
        };
      },
      ["warehouse_insert", "warehouse_exec", "warehouse_readback"]
    ],
    [
      "upsert_refusal: the host accepts the upsert",
      "upsert_refusal",
      (fake) => {
        fake.raw.warehouse.upsert = async () => ({});
      }
    ],
    [
      "tx_refusal: the host refuses in other words",
      "tx_refusal",
      (fake) => {
        fake.raw.tx = async () => {
          throw new Error("ctx.tx: database 'canary_warehouse' is not configured for this project");
        };
      }
    ],
    [
      "sql_read: query throws",
      "sql_read",
      (fake) => {
        fake.raw.query = async () => {
          throw new Error("no default database");
        };
      }
    ],
    [
      "sql_read: query answers a bare array instead of { rows, truncated }",
      "sql_read",
      (fake) => {
        fake.raw.query = async () => [{ one: 1 }];
      }
    ],
    [
      "sql_read: query answers no rows",
      "sql_read",
      (fake) => {
        fake.raw.query = async () => ({ rows: [], truncated: false });
      }
    ],
    [
      "sql_stream: the generator throws",
      "sql_stream",
      (fake) => {
        fake.raw.queryStream = async function* () {
          throw new Error("no default database");
        };
      }
    ],
    [
      "oltp_roundtrip: the read finds nothing",
      "oltp_roundtrip",
      (fake) => {
        fake.raw.oltp.query = async () => [];
      }
    ],
    [
      "oltp_transaction: begin throws",
      "oltp_transaction",
      (fake) => {
        fake.raw.oltp.tx = async () => {
          throw new Error("ctx.oltp.tx: could not open a transaction: too many connections");
        };
      }
    ],
    [
      "shape_zoo: a column reads back a different value",
      "shape_zoo",
      (fake) => {
        fake.state.corruptZoo = true;
      }
    ],
    [
      "org_read: places has the wrong shape",
      "org_read",
      (fake) => {
        fake.raw.org.places = async () => ({ places: "nope", total: 1 });
      }
    ],
    [
      "org_read: people has the wrong shape",
      "org_read",
      (fake) => {
        fake.raw.org.people = async () => ({ people: [PERSON], total: "1" });
      }
    ],
    [
      "org_read: an assignment lacks its fields",
      "org_read",
      (fake) => {
        fake.raw.org.assignments = async () => ({ assignments: [{ id: "assignment-2" }], total: 1 });
      }
    ],
    [
      "storage_roundtrip: put throws",
      "storage_roundtrip",
      (fake) => {
        fake.raw.storage.put = async () => {
          throw new Error("storage.write capability missing");
        };
      }
    ],
    [
      "storage_roundtrip: copy reports the wrong size",
      "storage_roundtrip",
      (fake) => {
        fake.raw.storage.copy = async (_fromKey, toPathname) => ({
          key: toPathname,
          size: 1,
          contentType: OCTET
        });
      }
    ],
    [
      "secrets_roundtrip: set throws",
      "secrets_roundtrip",
      (fake) => {
        fake.raw.secrets.set = async () => {
          throw new Error("secrets.write capability missing");
        };
      }
    ],
    [
      "check_in: fetch throws",
      "check_in",
      (fake) => {
        fake.raw.fetch = async () => {
          throw new Error("read failed");
        };
      }
    ]
  ];

  it.each(breakers)("%s → names the step", async (_label, step, breakIt, steps) => {
    const fake = makeFake();
    breakIt(fake);
    const err = await failure(run(fake, steps ?? [step]));
    expect(err.message.startsWith(`canary step ${step} failed: `)).toBe(true);
  });
});

describe("STEP_OPS", () => {
  /**
   * Steps a step reads after, run with it in the same run (`warehouse_readback`
   * expects what the run's own write steps sent). The fake is deterministic, so
   * their calls are counted on a second fake and skipped.
   */
  const PREREQUISITES: Partial<Record<StepName, StepName[]>> = {
    warehouse_readback: ["warehouse_insert", "warehouse_exec"]
  };

  it.each(ALL_STEPS)("%s declares exactly the host ops it makes, in first-call order", async (step) => {
    const before = PREREQUISITES[step] ?? [];
    let from = 0;
    if (before.length > 0) {
      const prelude = makeFake();
      await run(prelude, before);
      from = prelude.calls.length;
    }
    const fake = makeFake();
    await run(fake, [...before, step]);
    const made = [...new Set(fake.calls.slice(from).map((call) => call.member))];
    expect(made).toEqual([...STEP_OPS[step]]);
  });

  it("names every step once, and no step twice", () => {
    expect(Object.keys(STEP_OPS).sort()).toEqual([...ALL_STEPS].sort());
    for (const step of ALL_STEPS) {
      expect(new Set(STEP_OPS[step]).size, step).toBe(STEP_OPS[step].length);
    }
  });
});

describe("the bundle oxyc publish ships", () => {
  // `oxyc publish` bundles each function with esbuild (esm, platform neutral)
  // and the isolate runs it with no Node globals. A value import that drags in
  // React reads `process.env` at load and fails every run before a step starts,
  // so evaluate the real bundle with `process` shadowed.
  it.each(["canary", "echo"])("functions/%s.ts loads without `process`", async (name) => {
    const out = await build({
      entryPoints: [`functions/${name}.ts`],
      bundle: true,
      format: "esm",
      platform: "neutral",
      write: false,
      logLevel: "silent"
    });
    // The export statement is not the file's last text: license comments follow it.
    const script = out.outputFiles[0].text.replace(/^export\s*\{[^}]*\};?/m, "");
    expect(() => new Function("process", script)(undefined)).not.toThrow();
  });
});

describe("selectSteps", () => {
  it("returns the named steps", () => {
    expect(selectSteps("warehouse_insert,check_in")).toEqual(["warehouse_insert", "check_in"]);
  });

  it("tolerates whitespace and keeps check_in last whatever order the list names", () => {
    expect(selectSteps(" check_in , warehouse_insert ")).toEqual(["warehouse_insert", "check_in"]);
  });

  it("throws on an unknown step, naming it", () => {
    expect(() => selectSteps("nope")).toThrow(/nope/);
  });

  it("throws on a list that names no step, rather than running nothing", () => {
    expect(() => selectSteps(" , ")).toThrow(/names no step/);
  });

  it("returns every step when unset", () => {
    expect(selectSteps(undefined)).toEqual(ALL_STEPS);
  });
});
