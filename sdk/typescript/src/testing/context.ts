// `createTestContext(manifest, options)`: a typed `OxyFunctionContext` whose
// every member is the runtime's `__buildCtx` shape (`runtime.rs`), backed by
// in-memory stores and refusing exactly where — and in the words — the host
// refuses. Every host op the function makes is recorded as a `HostCall`.
//
// Each member mirrors `__buildCtx` one for one: the ctx member packs its
// arguments into the op's payload, the "host op" answers (a gate, a store, an
// override), and the member post-processes as the runtime does (`oltp.query`
// returns `r.rows`, `airhouse.append` returns `r.rowCount`, …). An `override`
// therefore replaces the HOST OP — gates included, as `fake.raw.warehouse.upsert
// = async () => ({})` did on the canary — and answers with the JSON the host
// would reply, not the value the ctx member returns.

import * as nodeCryptoImport from "node:crypto";
import type {
  OxyFunctionContext,
  OxyFunctionRow,
  OxyFunctionUser,
  OxyOrgAssignment,
  OxyOrgPlace,
  OxyTransaction
} from "../custom-app/function-context";
import { isSafeOutbound } from "./fetch-rules";
import { type FunctionGates, MAX_SCHEMA_NAME_LEN, type ManifestLike, readGates } from "./gates";
import {
  DIALECTS,
  type Dialect,
  FETCH_MAX_BYTES,
  type HostOp,
  MAX_OPEN_TRANSACTIONS,
  REFUSALS,
  SURFACES
} from "./host-contract";
import { contextError, hostError, isHostError, refuse } from "./host-error";
import { runWithoutIsolateGlobals } from "./run";
import { FetchStore, type StoredObject, TableStore, ZooRefusalError } from "./stores";
import type { RowSource } from "./zoo";

// Captured at load: the stores are "the host", so they keep Node's facilities
// even while `t.run(fn)` has removed them from the function under test.
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const nodeBtoa = globalThis.btoa;
const nodeAtob = globalThis.atob;

/** The two `node:crypto` members `ctx.crypto` needs.
 *
 *  A static import, not `process.getBuiltinModule("node:crypto")`: the module
 *  graph is evaluated once, long before any `t.run` can remove `process`, so
 *  the reason to reach for that API does not apply — and it costs Node 20.16,
 *  while this package says it runs on 18. */
interface NodeCryptoLike {
  createHmac(
    algorithm: string,
    key: string
  ): { update(data: string): { digest(encoding: "hex" | "base64"): string } };
  timingSafeEqual(a: Uint8Array, b: Uint8Array): boolean;
}
const nodeCryptoModule: NodeCryptoLike = nodeCryptoImport as unknown as NodeCryptoLike;

/** One database the project configures, as the context needs to know it. */
export interface TestDatabase {
  dialect: Dialect;
  /**
   * `destination_kind`'s mapping (`host/destinations.rs`): `airhouse` /
   * `airhouse_managed` may be written; `postgres_managed` is the org's OLTP
   * store, refused; anything else is a customer warehouse, read-only without a
   * `customerWarehouseWrites` reason. An unknown kind fails closed the same way.
   */
  kind: "airhouse" | "airhouse_managed" | "postgres_managed" | "customer";
}

export interface TestContextOptions {
  /** The `functions.<name>` entry whose capabilities gate the context. */
  function: string;
  /** The project's databases by name; `ctx.query` reaches `defaultDatabase` or the first. */
  databases?: Record<string, TestDatabase>;
  defaultDatabase?: string;
  /** `ctx.env`. */
  env?: Record<string, string>;
  /** `ctx.user`, over a default org member. */
  user?: Partial<OxyFunctionUser>;
}

/** One host op the function made, as the context recorded it. */
export interface HostCall {
  op: HostOp;
  /** The op's payload as `__buildCtx` packs it: `{ database, table, rows }`, `{ sql, params }`, … */
  args: Record<string, unknown>;
  /** `refused`: a `HostError` the context threw; `threw`: any other error, an override's included. */
  outcome: "ok" | "refused" | "threw";
  /** The full error message — `"<surface>: <host message>"` for a refusal. */
  message?: string;
  /** What the host op replied, on `ok`. */
  result?: unknown;
  /** For a read answered from a store: the weakest source among its rows. */
  source?: RowSource;
}

/** The host op behind a ctx member: answers with what the host would reply. */
export type HostOpImpl = (args: Record<string, unknown>) => Promise<unknown> | unknown;

type Person = { id: string; name: string; role: string | null; kind: "member" | "frontline" };

/** The stores, for a test to seed and inspect. */
export interface TestState {
  warehouse(database: string): TableStore;
  readonly oltp: TableStore;
  readonly airhouse: TableStore;
  readonly storage: Map<string, StoredObject>;
  readonly secrets: Map<string, string>;
  readonly emails: Record<string, unknown>[];
  readonly fetch: FetchStore;
  readonly org: { people: Person[]; places: OxyOrgPlace[]; assignments: OxyOrgAssignment[] };
  readonly semantic: { on(answer: (spec: Record<string, unknown>) => unknown): void };
  readonly airway: { runs: { pipelineRef: string; variables: unknown }[] };
}

export interface TestContext {
  ctx: OxyFunctionContext;
  /** Every host op made, in order. */
  readonly calls: readonly HostCall[];
  /** The de-duplicated first-call order of op names. */
  ops(): HostOp[];
  callsTo(op: HostOp): HostCall[];
  /** Replace one host op for this context; still recorded. */
  override(op: HostOp, impl: HostOpImpl): void;
  /** Evaluate `fn` with the isolate's absent globals absent (`run.ts`). */
  run<T>(fn: () => Promise<T> | T): Promise<T>;
  readonly state: TestState;
  readonly gates: FunctionGates;
}

const DEFAULT_USER: OxyFunctionUser = {
  id: "user-1",
  email: "author@example.test",
  orgId: "org-1",
  name: "Author",
  appRole: "member",
  orgRole: "member",
  teams: [],
  kind: "user",
  reach: { everywhere: true, via: "org-member", locations: [] }
};

export function createTestContext(
  manifest: ManifestLike,
  options: TestContextOptions
): TestContext {
  const gates = readGates(manifest, options.function);
  const manifestSlug = typeof manifest.slug === "string" ? manifest.slug : "";
  const databases = options.databases ?? {};
  const calls: HostCall[] = [];
  const overrides = new Map<HostOp, HostOpImpl>();
  const warehouses = new Map<string, TableStore>();
  let semanticAnswer: ((spec: Record<string, unknown>) => unknown) | null = null;

  const state: TestState = {
    warehouse(database) {
      const db = databases[database];
      if (!db) throw contextError(`no database "${database}" in options.databases`);
      let store = warehouses.get(database);
      if (!store) {
        store = new TableStore(db.dialect, "warehouse");
        warehouses.set(database, store);
      }
      return store;
    },
    oltp: new TableStore("postgres", "oltp", gates.writerSchema),
    airhouse: new TableStore("duckdb", "warehouse", gates.writerSchema),
    storage: new Map(),
    secrets: new Map(),
    emails: [],
    fetch: new FetchStore(),
    org: { people: [], places: [], assignments: [] },
    semantic: {
      on: (answer) => {
        semanticAnswer = answer;
      }
    },
    airway: { runs: [] }
  };

  /** Run one host op: the override or `impl`, recorded either way. */
  async function op(
    name: HostOp,
    args: Record<string, unknown>,
    impl: HostOpImpl
  ): Promise<unknown> {
    const call: HostCall = { op: name, args, outcome: "ok" };
    calls.push(call);
    try {
      const result = await (overrides.get(name) ?? impl)(args);
      if (result && typeof result === "object" && "__source" in result) {
        const { __source } = result as { __source: RowSource | null };
        if (__source) call.source = __source;
        const rest = Array.isArray(result)
          ? [...result]
          : Object.fromEntries(Object.entries(result).filter(([k]) => k !== "__source"));
        call.result = rest;
        return rest;
      }
      call.result = result;
      return result;
    } catch (err) {
      const thrown =
        err instanceof ZooRefusalError
          ? hostError(name.startsWith("tx.") ? SURFACES.tx : SURFACES.oltp, err.message)
          : err;
      call.outcome = isHostError(thrown) ? "refused" : "threw";
      call.message = thrown instanceof Error ? thrown.message : String(thrown);
      throw thrown;
    }
  }

  // ── gates, in the host's order ───────────────────────────────────────────

  /** `check_write_destination` then `destination_write_policy` (`host.rs`, `destinations.rs`). */
  function checkWriteDestination(
    surface: string,
    database: string,
    transaction: boolean
  ): TestDatabase {
    if (!gates.destinations.includes(database)) {
      throw refuse(surface, REFUSALS.destinationNotDeclared, { database });
    }
    const db = databases[database];
    if (!db) throw refuse(surface, REFUSALS.databaseNotConfigured, { database });
    switch (db.kind) {
      case "airhouse":
      case "airhouse_managed":
        return db;
      case "postgres_managed":
        throw refuse(surface, REFUSALS.managedOltpWrite, { database });
      default: {
        if (gates.customerWarehouseWrites[database]) return db;
        const words: string[] = [REFUSALS.customerWarehouseWrite.refusal];
        if (transaction) words.push(REFUSALS.customerWarehouseTransaction.refusal);
        throw hostError(surface, words.join("").replace(/\{database\}/g, database));
      }
    }
  }

  /**
   * The host's `WriterCapability` has three states, not two: enabled, disabled,
   * and enabled-but-the-slug-cannot-back-a-schema. Gating on the boolean alone
   * let an app whose slug derives no schema pass here and fail closed in
   * production — the exact shape this module exists to catch.
   */
  function requireWriterSchema(surface: string, store: "OLTP" | "Airhouse"): void {
    if (gates.writerSchema === null) {
      throw refuse(surface, REFUSALS.slugCannotBackASchema, {
        slug: manifestSlug,
        store,
        max: MAX_SCHEMA_NAME_LEN
      });
    }
  }

  function requireOltp(surface: string): TableStore {
    if (!gates.oltp) throw refuse(surface, REFUSALS.oltp);
    requireWriterSchema(surface, "OLTP");
    return state.oltp;
  }

  function requireAirhouse(): TableStore {
    if (!gates.airhouse) throw refuse(SURFACES.airhouse, REFUSALS.airhouse);
    requireWriterSchema(SURFACES.airhouse, "Airhouse");
    return state.airhouse;
  }

  function requireStorage(opName: string): void {
    const needsWrite = ["getUploadUrl", "put", "delete", "copy"].includes(opName);
    const needsRead = ["getDownloadUrl", "get", "head", "list", "copy"].includes(opName);
    if (needsWrite && !gates.storageWrite) throw refuse(SURFACES.storage, REFUSALS.storageWrite);
    if (needsRead && !gates.storageRead) throw refuse(SURFACES.storage, REFUSALS.storageRead);
  }

  function defaultDatabase(): string {
    const name = options.defaultDatabase ?? Object.keys(databases)[0];
    if (!name) throw refuse(SURFACES.query, REFUSALS.noDefaultDatabase);
    return name;
  }

  /** `oltp_statement`: `params` absent or an array, else refused. */
  function oltpParams(surface: string, params: unknown): unknown[] {
    if (params === undefined || params === null) return [];
    if (Array.isArray(params)) return params;
    throw refuse(surface, REFUSALS.oltpParamsNotArray);
  }

  const read = (store: TableStore, sql: string, params: unknown[] = []) => {
    const { rows, source } = store.query(sql, params);
    return { rows, truncated: false, __source: source };
  };

  // ── ctx.tx / ctx.oltp.tx: the runtime's `__runTx` bracket ────────────────

  let open = 0;
  let nextTxId = 0;

  async function runTx<T>(
    signature: string,
    label: string,
    beginOp: "tx.begin" | "tx.begin_oltp",
    beginPayload: Record<string, unknown>,
    fn: (tx: OxyTransaction) => Promise<T> | T
  ): Promise<T> {
    if (typeof fn !== "function") {
      throw new TypeError(
        REFUSALS.transactionFnNotFunction.refusal.replace("{signature}", signature)
      );
    }
    let store: TableStore | null = null;
    // `open++` lives inside the begin impl, which `t.override("tx.begin", …)`
    // replaces — so the unconditional `open--` below used to run for a
    // transaction that never incremented, leaving the counter negative and the
    // MAX_OPEN refusal permanently disarmed for the rest of the context.
    let incremented = false;
    const { id } = (await op(beginOp, beginPayload, () => {
      if (beginOp === "tx.begin") {
        const database = String(beginPayload.database);
        const db = checkWriteDestination(SURFACES.tx, database, true);
        if (!DIALECTS[db.dialect].opensTransactions) {
          const cause = REFUSALS.transactionUnsupported.refusal.replace(
            "{backend}",
            DIALECTS[db.dialect].name
          );
          throw refuse(SURFACES.tx, REFUSALS.transactionOpen, { database, e: cause });
        }
        store = state.warehouse(database);
      } else {
        store = requireOltp(SURFACES.tx);
      }
      if (open >= MAX_OPEN_TRANSACTIONS) {
        throw refuse(SURFACES.tx, REFUSALS.transactionsOpen, { MAX_OPEN: MAX_OPEN_TRANSACTIONS });
      }
      open++;
      incremented = true;
      return { id: ++nextTxId };
    })) as { id: number };
    // `store` is set inside the begin op; an override of `tx.begin` skips that,
    // so fall back to the store the payload names.
    const table: TableStore =
      (store as TableStore | null) ??
      (beginOp === "tx.begin_oltp"
        ? state.oltp
        : databases[String(beginPayload.database)]
          ? state.warehouse(String(beginPayload.database))
          : new TableStore("postgres", "warehouse"));
    const restore = table.snapshot();
    let closed = false;
    const live = (what: string) => {
      if (closed) {
        throw hostError(
          label,
          REFUSALS.transactionFinished.refusal.replace("{label}", label).replace("{what}", what)
        );
      }
    };
    const handle: OxyTransaction = {
      query: async (sql, params) => {
        live("query");
        const r = (await op("tx.query", { id, sql: String(sql), params: params ?? [] }, (a) =>
          read(table, String(a.sql), oltpParams(SURFACES.tx, a.params))
        )) as { rows: OxyFunctionRow[] };
        return r.rows;
      },
      exec: async (sql, params) => {
        live("exec");
        const r = (await op("tx.exec", { id, sql: String(sql), params: params ?? [] }, (a) => ({
          rowCount: table.exec(String(a.sql), oltpParams(SURFACES.tx, a.params))
        }))) as { rowCount: number };
        return r.rowCount;
      }
    };
    let result: T;
    try {
      result = await fn(handle);
    } catch (err) {
      closed = true;
      if (incremented) open--;
      try {
        await op("tx.rollback", { id }, () => {
          restore();
          return {};
        });
      } catch {
        // Swallowed, as `__runTx` swallows it: the author's error is the one to see.
      }
      throw err;
    }
    closed = true;
    if (incremented) open--;
    await op("tx.commit", { id }, () => ({}));
    return result;
  }

  // ── ctx ──────────────────────────────────────────────────────────────────

  const ctx: OxyFunctionContext = {
    user: { ...DEFAULT_USER, ...options.user },
    env: { ...(options.env ?? {}) },
    log: () => {},
    crypto: nodeCrypto(),
    query: async (sql) =>
      (await op("query", { sql: String(sql) }, (a) =>
        read(state.warehouse(defaultDatabase()), String(a.sql))
      )) as {
        rows: OxyFunctionRow[];
        truncated: boolean;
      },
    queryStream: async function* (sql, opts) {
      const batchSize = opts?.batchSize || 1000;
      const rows = (await op("query_stream", { sql: String(sql) }, (a) => {
        const { rows, __source } = read(state.warehouse(defaultDatabase()), String(a.sql));
        return Object.assign(rows, { __source });
      })) as OxyFunctionRow[];
      for (let i = 0; i < rows.length; i += batchSize) yield rows.slice(i, i + batchSize);
    },
    fetch: (url, init) =>
      op("fetch", { url, init: init ?? {} }, (a) =>
        fetchOp(String(a.url), (a.init ?? {}) as NonNullable<typeof init>)
      ) as ReturnType<OxyFunctionContext["fetch"]>,
    warehouse: {
      insert: (database, table, rows) =>
        op("warehouse.insert", { database, table, rows }, (a) => {
          checkWriteDestination(SURFACES.warehouse, String(a.database), false);
          state.warehouse(String(a.database)).append(String(a.table), a.rows as OxyFunctionRow[]);
          return { ok: true };
        }),
      exec: (database, sql) =>
        op("warehouse.exec", { database, sql }, (a) => {
          checkWriteDestination(SURFACES.warehouse, String(a.database), false);
          state.warehouse(String(a.database)).exec(String(a.sql));
          return { ok: true };
        }),
      upsert: (database, table, rows, conflictColumns) =>
        op("warehouse.upsert", { database, table, rows, conflictColumns }, (a) => {
          const database = String(a.database);
          const db = checkWriteDestination(SURFACES.warehouse, database, false);
          if (!DIALECTS[db.dialect].parsesOnConflict) {
            throw refuse(SURFACES.warehouse, REFUSALS.upsertUnsupported, {
              dialect: DIALECTS[db.dialect].name
            });
          }
          if (db.kind === "airhouse" || db.kind === "airhouse_managed") {
            // In the CONTEXT'S OWN WORDS, not the host's (§7, answer 6): the host lets
            // this statement through and DuckLake refuses it at runtime, because a
            // DuckLake table cannot carry the primary key ON CONFLICT needs. A fake
            // that resolved here would pass where production fails.
            throw contextError(
              `@oxy-hq/sdk/testing: ctx.warehouse.upsert on "${database}" would be refused by the ENGINE, not the host. Airhouse is DuckLake, and a DuckLake table cannot carry the primary key or unique constraint that INSERT … ON CONFLICT needs, so the statement the host lets through fails at runtime. Use ctx.airhouse.append and keep one row per id on read. (These are the test context's words, not a host refusal — assert that the call was refused, not on this text, which may be reworded.)`
            );
          }
          state.warehouse(database).append(String(a.table), a.rows as OxyFunctionRow[]);
          return { ok: true };
        }),
      query: (database, sql) =>
        op("warehouse.query", { database, sql }, (a) => {
          const database = String(a.database);
          if (!databases[database])
            throw refuse(SURFACES.warehouse, REFUSALS.databaseNotConfigured, { database });
          return read(state.warehouse(database), String(a.sql));
        }) as Promise<{ rows: OxyFunctionRow[]; truncated: boolean }>
    },
    tx: (database, fn) =>
      runTx("ctx.tx(database, fn)", SURFACES.tx, "tx.begin", { database: String(database) }, fn),
    oltp: {
      tx: (fn) => runTx("ctx.oltp.tx(fn)", "ctx.oltp.tx", "tx.begin_oltp", {}, fn),
      query: async (sql, params) => {
        const r = (await op("oltp.query", { sql: String(sql), params: params ?? [] }, (a) =>
          read(requireOltp(SURFACES.oltp), String(a.sql), oltpParams(SURFACES.oltp, a.params))
        )) as { rows: OxyFunctionRow[] };
        return r.rows;
      },
      exec: async (sql, params) => {
        const r = (await op("oltp.exec", { sql: String(sql), params: params ?? [] }, (a) => ({
          rowCount: requireOltp(SURFACES.oltp).exec(
            String(a.sql),
            oltpParams(SURFACES.oltp, a.params)
          )
        }))) as { rowCount: number };
        return r.rowCount;
      }
    },
    airhouse: {
      schema: gates.airhouse ? gates.writerSchema : null,
      query: (sql) =>
        op("airhouse.query", { sql: String(sql) }, (a) =>
          read(requireAirhouse(), String(a.sql))
        ) as Promise<{
          rows: OxyFunctionRow[];
          truncated: boolean;
        }>,
      exec: async (sql) => {
        await op("airhouse.exec", { sql: String(sql) }, (a) => {
          requireAirhouse().exec(String(a.sql));
          return { ok: true };
        });
      },
      append: async (table, rows) => {
        const r = (await op("airhouse.append", { table: String(table), rows }, (a) => ({
          rowCount: requireAirhouse().append(String(a.table), a.rows as OxyFunctionRow[])
        }))) as { rowCount: number };
        return r.rowCount;
      }
    },
    secrets: {
      set: async (key, value) => {
        await op("secrets.set", { key: String(key), value: String(value) }, (a) => {
          if (!gates.secretsWrite) throw refuse(SURFACES.secretsSet, REFUSALS.secretsWrite);
          state.secrets.set(String(a.key), String(a.value));
          return null;
        });
      }
    },
    email: {
      send: (input) =>
        op("email.send", { ...input }, (a) => {
          if (!gates.emailSend) throw refuse(SURFACES.emailSend, REFUSALS.emailSend);
          state.emails.push(a);
          return { messageId: `message-${state.emails.length}` };
        }) as Promise<{ messageId: string }>
    },
    org: {
      people: () =>
        op("org.people", {}, () => {
          if (!gates.orgRead) throw refuse(SURFACES.orgPeople, REFUSALS.orgPeople);
          return { people: [...state.org.people], total: state.org.people.length };
        }) as ReturnType<OxyFunctionContext["org"]["people"]>,
      places: () =>
        op("org.places", {}, () => {
          if (!gates.orgRead)
            throw refuse(SURFACES.orgPlaces, REFUSALS.orgMember, { member: "places" });
          return { total: state.org.places.length, places: [...state.org.places] };
        }) as Promise<{ places: OxyOrgPlace[]; total: number }>,
      assignments: () =>
        op("org.assignments", {}, () => {
          if (!gates.orgRead)
            throw refuse(SURFACES.orgAssignments, REFUSALS.orgMember, { member: "assignments" });
          return { total: state.org.assignments.length, assignments: [...state.org.assignments] };
        }) as Promise<{ assignments: OxyOrgAssignment[]; total: number }>
    },
    storage: storageApi(),
    semantic: {
      query: (spec) =>
        op("semantic.query", { spec }, (a) => {
          if (!semanticAnswer) {
            throw contextError(
              "ctx.semantic.query has no answer registered: the zoo pins no semantic payload, so supply one with t.state.semantic.on((spec) => result)"
            );
          }
          return semanticAnswer(a.spec as Record<string, unknown>);
        })
    },
    airway: {
      run: (pipelineRef, variables) =>
        op(
          "airway.run",
          { pipelineRef: String(pipelineRef), variables: variables ?? null },
          (a) => {
            state.airway.runs.push({ pipelineRef: String(a.pipelineRef), variables: a.variables });
            return { runId: `run-${state.airway.runs.length}` };
          }
        ) as Promise<{ runId: string }>
    }
  };

  // ── ctx.fetch: `is_safe_outbound`, headers, the byte cap ─────────────────

  function fetchOp(url: string, init: RequestInit & { encoding?: "utf8" | "base64" }) {
    let parsed: URL;
    try {
      parsed = new URL(url);
    } catch (e) {
      throw refuse(SURFACES.fetch, REFUSALS.fetchInvalidUrl, {
        e: e instanceof Error ? e.message : String(e)
      });
    }
    if (!isSafeOutbound(parsed)) throw refuse(SURFACES.fetch, REFUSALS.fetchBlocked, { url });
    for (const [k, v] of Object.entries(headersOf(init.headers))) {
      if (!/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(k))
        throw refuse(SURFACES.fetch, REFUSALS.fetchInvalidHeaderName, { k });
      if (/[\r\n\0]/.test(v)) throw refuse(SURFACES.fetch, REFUSALS.fetchInvalidHeaderValue, { k });
    }
    state.fetch.received.push({ url, init });
    const answer = state.fetch.answerFor(url);
    if (answer === undefined) {
      throw contextError(
        `ctx.fetch has no answer for ${url}: register one with t.state.fetch.on(url, { status, body }) — nothing here reaches the network`
      );
    }
    const reply = typeof answer === "function" ? answer(url, init) : answer;
    const fixture = reply.body ?? "";
    const fixtureIsBase64 = reply.encoding === "base64";
    const max = gates.fetchMaxBytes ?? FETCH_MAX_BYTES;
    const len = fixtureIsBase64 ? base64ByteLength(fixture) : encoder.encode(fixture).length;
    if (len > max) throw refuse(SURFACES.fetch, REFUSALS.fetchTooLarge, { len, max_bytes: max });
    // The host encodes the bytes it received (`host.rs`): `utf8` is a lossy
    // string of them, `base64` is them encoded. It does not hand back what it
    // was given under a different label — reading a binary fixture as utf8, or
    // a text one as base64, changes the body. `ctx.storage.get` converts both
    // ways, and this is the gotcha `product-context.md` names, so convert here.
    if (init.encoding !== undefined && init.encoding !== "utf8" && init.encoding !== "base64") {
      throw contextError(
        `ctx.fetch: unknown encoding '${String(init.encoding)}' (expected 'utf8' or 'base64')`
      );
    }
    const wantBase64 = init.encoding === "base64";
    const body = wantBase64
      ? fixtureIsBase64
        ? fixture
        : utf8ToBase64(fixture)
      : fixtureIsBase64
        ? base64ToUtf8(fixture)
        : fixture;
    return { status: reply.status ?? 200, body, encoding: wantBase64 ? "base64" : "utf8" };
  }

  // ── ctx.storage: a Map with an offset cursor, as the canary's fake was ────

  function storageApi(): OxyFunctionContext["storage"] {
    const now = () => new Date().toISOString();
    const put = (key: string, body: string, contentType: string | null): StoredObject => {
      const object = {
        key,
        body,
        contentType,
        size: base64ByteLength(body),
        lastModified: now()
      };
      state.storage.set(key, object);
      return object;
    };
    const storageOp = (name: string, args: Record<string, unknown>, impl: HostOpImpl) =>
      op(`storage.${name}` as HostOp, args, (a) => {
        requireStorage(name);
        return impl(a);
      });
    return {
      getUploadUrl: (opts) =>
        storageOp("getUploadUrl", { ...opts }, (a) => {
          const pathname = String(a.pathname ?? `uploads/${String(a.filename ?? "upload")}`);
          const key = `${pathname.replace(/(\.[^.]*)?$/, `-${randomSuffix()}$1`)}`;
          return { url: `https://storage.example.test/put/${key}`, key, expiresAt: now() };
        }) as ReturnType<OxyFunctionContext["storage"]["getUploadUrl"]>,
      getDownloadUrl: (key, opts) =>
        storageOp("getDownloadUrl", { key: String(key), ...opts }, (a) => ({
          url: `https://storage.example.test/get/${String(a.key)}`,
          expiresAt: now()
        })) as ReturnType<OxyFunctionContext["storage"]["getDownloadUrl"]>,
      put: (pathname, body, opts) =>
        storageOp("put", { pathname: String(pathname), body: String(body), ...opts }, (a) => {
          const key = a.addRandomSuffix
            ? String(a.pathname).replace(/(\.[^.]*)?$/, `-${randomSuffix()}$1`)
            : String(a.pathname);
          if (state.storage.has(key) && !a.allowOverwrite) {
            throw hostError(
              SURFACES.storage,
              `put: "${key}" already exists (pass allowOverwrite: true)`
            );
          }
          const base64 = a.encoding === "base64" ? String(a.body) : utf8ToBase64(String(a.body));
          const stored = put(key, base64, (a.contentType as string | undefined) ?? null);
          return {
            key,
            size: stored.size,
            contentType: stored.contentType ?? "application/octet-stream"
          };
        }) as ReturnType<OxyFunctionContext["storage"]["put"]>,
      get: (key, opts) =>
        storageOp("get", { key: String(key), ...opts }, (a) => {
          const object = state.storage.get(String(a.key));
          if (!object) return null;
          const base64 = a.encoding === "base64";
          return {
            body: base64 ? object.body : base64ToUtf8(object.body),
            contentType: object.contentType,
            size: object.size,
            encoding: base64 ? "base64" : "utf8"
          };
        }) as ReturnType<OxyFunctionContext["storage"]["get"]>,
      head: (key) =>
        storageOp("head", { key: String(key) }, (a) => {
          const object = state.storage.get(String(a.key));
          return object
            ? {
                key: object.key,
                size: object.size,
                contentType: object.contentType,
                lastModified: object.lastModified
              }
            : null;
        }) as ReturnType<OxyFunctionContext["storage"]["head"]>,
      list: (opts) =>
        storageOp("list", { ...opts }, (a) => {
          const prefix = String(a.prefix ?? "");
          const limit = Number(a.limit ?? 1000);
          const offset = a.cursor ? Number(a.cursor) : 0;
          const keys = [...state.storage.keys()].filter((k) => k.startsWith(prefix)).sort();
          const page = keys.slice(offset, offset + limit).map((k) => {
            const o = state.storage.get(k) as StoredObject;
            return {
              key: o.key,
              size: o.size,
              contentType: o.contentType,
              lastModified: o.lastModified
            };
          });
          const hasMore = offset + limit < keys.length;
          return { objects: page, cursor: hasMore ? String(offset + limit) : null, hasMore };
        }) as ReturnType<OxyFunctionContext["storage"]["list"]>,
      delete: (keyOrKeys) =>
        storageOp(
          "delete",
          Array.isArray(keyOrKeys) ? { keys: keyOrKeys.map(String) } : { key: String(keyOrKeys) },
          (a) => {
            const keys = Array.isArray(a.keys) ? (a.keys as string[]) : [String(a.key)];
            for (const k of keys) state.storage.delete(k);
            return { deleted: keys.length };
          }
        ) as ReturnType<OxyFunctionContext["storage"]["delete"]>,
      copy: (fromKey, toPathname, opts) =>
        storageOp(
          "copy",
          { fromKey: String(fromKey), toPathname: String(toPathname), ...opts },
          (a) => {
            const from = state.storage.get(String(a.fromKey));
            if (!from)
              throw hostError(SURFACES.storage, `copy: "${String(a.fromKey)}" does not exist`);
            const key = String(a.toPathname);
            if (state.storage.has(key) && !a.allowOverwrite) {
              throw hostError(
                SURFACES.storage,
                `copy: "${key}" already exists (pass allowOverwrite: true)`
              );
            }
            const stored = put(key, from.body, from.contentType);
            return {
              key,
              size: stored.size,
              contentType: stored.contentType ?? "application/octet-stream"
            };
          }
        ) as ReturnType<OxyFunctionContext["storage"]["copy"]>
    };
  }

  return {
    ctx,
    calls,
    ops: () => [...new Set(calls.map((c) => c.op))],
    callsTo: (name) => calls.filter((c) => c.op === name),
    override: (name, impl) => {
      overrides.set(name, impl);
    },
    run: runWithoutIsolateGlobals,
    state,
    gates
  };
}

function headersOf(headers: HeadersInit | undefined): Record<string, string> {
  if (!headers) return {};
  if (Array.isArray(headers)) return Object.fromEntries(headers);
  if (typeof Headers !== "undefined" && headers instanceof Headers)
    return Object.fromEntries(headers.entries());
  return headers as Record<string, string>;
}

function randomSuffix(): string {
  return Math.random().toString(36).slice(2, 8);
}

/**
 * Bytes a base64 string decodes to. `length * 3 / 4` overstates by the padding,
 * which made `size` wrong by up to two bytes and fired the `ctx.fetch` cap early.
 */
function base64ByteLength(base64: string): number {
  const s = base64.trim();
  if (s === "") return 0;
  const padding = s.endsWith("==") ? 2 : s.endsWith("=") ? 1 : 0;
  return Math.floor((s.length * 3) / 4) - padding;
}

function utf8ToBase64(text: string): string {
  const bytes = encoder.encode(text);
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return nodeBtoa(binary);
}

function base64ToUtf8(base64: string): string {
  const binary = nodeAtob(base64);
  const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
  return decoder.decode(bytes);
}

/**
 * `ctx.crypto` over `node:crypto`: the same three synchronous members
 * `__buildCtx` binds, with the host's argument rules (`key` and `data`
 * required; `signature` and both sides of `timingSafeEqual` lenient). A digest
 * is ASCII, so the byte views need no `Buffer` — which `t.run` removes.
 */
function nodeCrypto(): OxyFunctionContext["crypto"] {
  const lib = (): NodeCryptoLike => nodeCryptoModule;
  const ascii = (s: string) => Uint8Array.from(s, (c) => c.charCodeAt(0));
  const digest = (algorithm: string, key: string, data: string, encoding: "hex" | "base64") =>
    lib().createHmac(algorithm, key).update(data).digest(encoding);
  const same = (a: string, b: string) =>
    a !== "" && b !== "" && a.length === b.length && lib().timingSafeEqual(ascii(a), ascii(b));
  return {
    hmac: ({ algorithm, key, data, encoding }) => {
      if (key == null)
        throw new TypeError("ctx.crypto.hmac: `key` is required — is the secret set?");
      if (data == null) throw new TypeError("ctx.crypto.hmac: `data` is required");
      return digest(algorithm ?? "sha256", String(key), String(data), encoding ?? "hex");
    },
    verifyHmac: ({ algorithm, key, data, signature, encoding }) => {
      if (key == null)
        throw new TypeError("ctx.crypto.verifyHmac: `key` is required — is the secret set?");
      if (data == null) throw new TypeError("ctx.crypto.verifyHmac: `data` is required");
      const expected = digest(algorithm ?? "sha256", String(key), String(data), encoding ?? "hex");
      return same(String(signature ?? ""), expected);
    },
    timingSafeEqual: (a, b) => same(a == null ? "" : String(a), b == null ? "" : String(b))
  };
}
