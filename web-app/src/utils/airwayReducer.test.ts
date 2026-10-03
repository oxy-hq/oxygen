import { describe, expect, it } from "vitest";

import type { AirwayEvent, ContractMutability, ResourceContract } from "@/services/api/airway";

import { reduceAirwayEvents } from "./airwayReducer";

// ── helpers ───────────────────────────────────────────────────────────────────

const ev = <T extends AirwayEvent["type"]>(
  type: T,
  payload: Extract<AirwayEvent, { type: T }>["payload"]
): AirwayEvent => ({ type, payload }) as AirwayEvent;

const PIPE = "shopify_users_orders";
const LOAD = "load-abc1";

const loadStarted = () => ev("load_started", { pipeline_name: PIPE, load_id: LOAD });
/** Minimal contract payload — only `mutability` matters to the reducer. */
const contract = (resource: string, mutability: ContractMutability): ResourceContract => ({
  resource,
  mutability,
  version_field: null,
  version_column: null,
  cursor_tracks_modification: null,
  restatement_window_ms: null,
  cursor_lag_ms: null,
  rewind_ms: null,
  requires_partition_repull: null
});
const extract = (table: string, rows: number) =>
  ev("extract_completed", {
    pipeline_name: PIPE,
    load_id: LOAD,
    table,
    rows_extracted: rows
  });
const normalize = (table: string, rows: number, child_tables: string[] = []) =>
  ev("normalize_completed", {
    pipeline_name: PIPE,
    load_id: LOAD,
    table,
    rows_normalized: rows,
    child_tables
  });
const destStart = (tables: string[]) =>
  ev("destination_load_started", {
    pipeline_name: PIPE,
    load_id: LOAD,
    tables
  });
const loadCompleted = (rows_loaded: Record<string, number>, duration_ms = 1234) =>
  ev("load_completed", {
    pipeline_name: PIPE,
    load_id: LOAD,
    tables: Object.keys(rows_loaded),
    rows_loaded,
    duration_ms
  });

// ── tests ─────────────────────────────────────────────────────────────────────

describe("reduceAirwayEvents", () => {
  it("empty stream → pending phases, no resources, running", () => {
    const v = reduceAirwayEvents([]);
    expect(v.phase).toEqual({
      extract: "pending",
      normalize: "pending",
      load: "pending"
    });
    expect(v.resources).toEqual([]);
    expect(v.status).toBe("running");
  });

  it("load_started seeds pipeline name + load id and activates extract", () => {
    const v = reduceAirwayEvents([loadStarted()]);
    expect(v.pipelineName).toBe(PIPE);
    expect(v.loadId).toBe(LOAD);
    expect(v.phase.extract).toBe("active");
    expect(v.status).toBe("running");
  });

  it("happy path: extract → normalize (+child) → load → completed", () => {
    const v = reduceAirwayEvents([
      loadStarted(),
      extract("users", 1204),
      extract("orders", 3910),
      normalize("users", 1204),
      normalize("orders", 3910, ["orders__items"]),
      destStart(["users", "orders", "orders__items"]),
      loadCompleted({ users: 1204, orders: 3910, orders__items: 8233 }, 4242)
    ]);

    expect(v.status).toBe("done");
    expect(v.durationMs).toBe(4242);
    expect(v.phase).toEqual({
      extract: "done",
      normalize: "done",
      load: "done"
    });

    const users = v.resources.find((r) => r.table === "users" && !r.parent);
    expect(users).toMatchObject({
      rowsExtracted: 1204,
      rowsNormalized: 1204,
      rowsLoaded: 1204,
      status: "done"
    });

    // Child row nests directly after its parent.
    const idxOrders = v.resources.findIndex((r) => r.table === "orders");
    const idxChild = v.resources.findIndex((r) => r.table === "orders__items");
    expect(idxChild).toBe(idxOrders + 1);
    // `NormalizeCompleted.rows_normalized` is the *parent's* count;
    // child tables only get a row count at load time, so the child
    // row carries `rowsLoaded` (from `load_completed.rows_loaded`) but
    // no `rowsNormalized`.
    expect(v.resources[idxChild]).toMatchObject({
      parent: "orders",
      rowsLoaded: 8233,
      status: "done"
    });
    expect(v.resources[idxChild].rowsNormalized).toBeUndefined();
  });

  it("is idempotent over a prefix (re-reduce yields the same view)", () => {
    const stream: AirwayEvent[] = [loadStarted(), extract("users", 10), normalize("users", 10)];
    const a = reduceAirwayEvents(stream);
    const b = reduceAirwayEvents([...stream]);
    expect(a).toEqual(b);
    // Prefix is a strict subset of the full view's resources.
    const full = reduceAirwayEvents([
      ...stream,
      destStart(["users"]),
      loadCompleted({ users: 10 })
    ]);
    expect(full.resources.map((r) => r.table)).toEqual(a.resources.map((r) => r.table));
  });

  it("pipeline_error marks the run failed and non-done rows error", () => {
    const v = reduceAirwayEvents([
      loadStarted(),
      extract("users", 5),
      normalize("users", 5),
      ev("pipeline_error", {
        pipeline_name: PIPE,
        load_id: LOAD,
        error: "boom at destination"
      })
    ]);
    expect(v.status).toBe("failed");
    expect(v.error).toBe("boom at destination");
    expect(v.resources.find((r) => r.table === "users")?.status).toBe("error");
  });

  it("task_failed (pre-processing failure) marks the run failed with the reason", () => {
    // Coordinator-level failure before the engine runs (e.g. secret /
    // connector / destination resolution in execute_airway). No engine
    // pipeline_error is emitted on this path — task_failed must still
    // surface the cause so the run page isn't blank.
    const v = reduceAirwayEvents([
      loadStarted(),
      ev("task_failed", {
        task_id: "t-1",
        attempt: 0,
        spec_kind: "airway",
        step_name: null,
        error: "secret `TOAST_CLIENT_SECRET` not found"
      })
    ]);
    expect(v.status).toBe("failed");
    expect(v.error).toBe("secret `TOAST_CLIENT_SECRET` not found");
  });

  it("resource_failed skips the resource; run completes with errors", () => {
    const resourceFailed = ev("resource_failed", {
      pipeline_name: PIPE,
      load_id: LOAD,
      table: "dining_options",
      error: "HTTP 403 Forbidden"
    });
    const v = reduceAirwayEvents([
      loadStarted(),
      extract("orders", 10),
      resourceFailed,
      normalize("orders", 10),
      destStart(["orders"]),
      loadCompleted({ orders: 10 })
    ]);
    expect(v.status).toBe("completed_with_errors");
    expect(v.failedResources).toEqual([{ table: "dining_options", error: "HTTP 403 Forbidden" }]);
    const failed = v.resources.find((r) => r.table === "dining_options");
    expect(failed?.status).toBe("error");
    expect(failed?.error).toBe("HTTP 403 Forbidden");
    // The skipped resource must not be flipped to done by load_completed.
    expect(v.resources.find((r) => r.table === "orders")?.status).toBe("done");
  });

  /**
   * **A run where EVERY resource failed is `failed`, not `completed_with_errors`
   * — even though `load_completed` arrives first.**
   *
   * airway 0.1.24 (#107) made a zero-of-N run return an error rather than a
   * partial success. On its streaming path the engine still emits
   * `load_completed` (the audit row and cursors are already settled) and only
   * then surfaces the failure as `pipeline_error`, so the terminal status
   * depends on the *later* event winning. That ordering is load-bearing here:
   * the "Reconnect QuickBooks" affordance on `/airway` is gated on
   * `status === "failed" && error`, and an expired `invalid_grant` token is
   * exactly the case that fails every resource for one reason.
   *
   * The sibling test above pins the opposite direction — one resource skipped
   * out of two really is `completed_with_errors` — so this cannot be satisfied
   * by making every skip a hard failure.
   */
  it("all resources failed → failed, even though load_completed precedes pipeline_error", () => {
    const failed = (table: string) =>
      ev("resource_failed", {
        pipeline_name: PIPE,
        load_id: LOAD,
        table,
        error: "invalid_grant: Token expired or revoked"
      });
    const v = reduceAirwayEvents([
      loadStarted(),
      failed("invoices"),
      failed("bills"),
      // Emitted before the error on the streaming path — nothing loaded, so
      // `rows_loaded` is empty.
      loadCompleted({}),
      ev("pipeline_error", {
        pipeline_name: PIPE,
        load_id: LOAD,
        error: "every resource failed — no data was loaded"
      })
    ]);
    expect(v.status).toBe("failed");
    expect(v.error).toBe("every resource failed — no data was loaded");
    expect(v.failedResources.map((f) => f.table)).toEqual(["invoices", "bills"]);
  });

  it("pipeline_plan renders the full skeleton up-front", () => {
    const v = reduceAirwayEvents([
      loadStarted(),
      ev("pipeline_plan", {
        pipeline_name: PIPE,
        load_id: LOAD,
        resources: ["users", "orders", "events"],
        destination: "my_warehouse"
      })
    ]);
    expect(v.destination).toBe("my_warehouse");
    expect(v.resources.map((r) => r.table)).toEqual(["users", "orders", "events"]);
    expect(v.resources.every((r) => r.status === "pending")).toBe(true);

    // A later extract_completed advances an already-listed row, not a dup.
    const v2 = reduceAirwayEvents([
      loadStarted(),
      ev("pipeline_plan", {
        pipeline_name: PIPE,
        load_id: LOAD,
        resources: ["users", "orders"],
        destination: "wh"
      }),
      extract("users", 4)
    ]);
    expect(v2.resources.map((r) => r.table)).toEqual(["users", "orders"]);
    expect(v2.resources.find((r) => r.table === "users")?.rowsExtracted).toBe(4);
  });

  it("pipeline_plan attaches each resource's contract by name", () => {
    const v = reduceAirwayEvents([
      loadStarted(),
      ev("pipeline_plan", {
        pipeline_name: PIPE,
        load_id: LOAD,
        // Deliberately not in `resources` order — attachment is by name.
        contracts: [contract("orders", "immutable"), contract("users", "undeclared")],
        resources: ["users", "orders"],
        destination: "wh"
      })
    ]);
    const byTable = new Map(v.resources.map((r) => [r.table, r]));
    expect(byTable.get("orders")?.contract?.mutability).toBe("immutable");
    // Declared-nothing is carried through as `undeclared` — a state, not a
    // missing value, and never silently the `opaque` airway defaults to.
    expect(byTable.get("users")?.contract?.mutability).toBe("undeclared");
  });

  it("leaves the contract unset when the stream carries none", () => {
    // A run recorded before contracts rode on `pipeline_plan`. "We were not
    // told" must stay distinguishable from "the connector declared nothing".
    const legacy = reduceAirwayEvents([
      loadStarted(),
      ev("pipeline_plan", {
        pipeline_name: PIPE,
        load_id: LOAD,
        resources: ["users"],
        destination: "wh"
      })
    ]);
    expect(legacy.resources[0].contract).toBeUndefined();

    // Same for a normalized child table: it is not a source resource, so no
    // contract is invented for it from its parent's.
    const withChild = reduceAirwayEvents([
      loadStarted(),
      ev("pipeline_plan", {
        pipeline_name: PIPE,
        load_id: LOAD,
        contracts: [contract("orders", "immutable")],
        resources: ["orders"],
        destination: "wh"
      }),
      normalize("orders", 2, ["orders__checks"])
    ]);
    const child = withChild.resources.find((r) => r.table === "orders__checks");
    expect(child?.parent).toBe("orders");
    expect(child?.contract).toBeUndefined();
  });

  it("ignores a contract for a resource the plan does not list", () => {
    const v = reduceAirwayEvents([
      loadStarted(),
      ev("pipeline_plan", {
        pipeline_name: PIPE,
        load_id: LOAD,
        contracts: [contract("ghost", "opaque")],
        resources: ["users"],
        destination: "wh"
      })
    ]);
    expect(v.resources.map((r) => r.table)).toEqual(["users"]);
    expect(v.resources[0].contract).toBeUndefined();
  });

  it("extract_started / normalize_started surface in-flight rows + phases", () => {
    const v = reduceAirwayEvents([
      loadStarted(),
      ev("extract_started", { pipeline_name: PIPE, load_id: LOAD, table: "orders" }),
      ev("extract_started", { pipeline_name: PIPE, load_id: LOAD, table: "users" })
    ]);
    expect(v.phase.extract).toBe("active");
    expect(v.resources.map((r) => r.table).sort()).toEqual(["orders", "users"]);
    expect(v.resources.every((r) => r.status === "extracting")).toBe(true);

    const v2 = reduceAirwayEvents([
      loadStarted(),
      ev("extract_started", { pipeline_name: PIPE, load_id: LOAD, table: "orders" }),
      extract("orders", 3),
      ev("normalize_started", { pipeline_name: PIPE, load_id: LOAD, table: "orders" })
    ]);
    expect(v2.phase.extract).toBe("done");
    expect(v2.phase.normalize).toBe("active");
    expect(v2.resources.find((r) => r.table === "orders")?.status).toBe("normalizing");
  });

  it("extract_started does not downgrade an already-advanced row", () => {
    const v = reduceAirwayEvents([
      loadStarted(),
      extract("orders", 5),
      normalize("orders", 5),
      // A late/duplicate extract_started must not knock it back.
      ev("extract_started", { pipeline_name: PIPE, load_id: LOAD, table: "orders" })
    ]);
    expect(v.resources.find((r) => r.table === "orders")?.status).toBe("normalizing");
  });

  it("per-table load events advance rows individually before load_completed", () => {
    const v = reduceAirwayEvents([
      loadStarted(),
      extract("users", 2),
      normalize("users", 2),
      destStart(["users"]),
      ev("table_load_started", { pipeline_name: PIPE, load_id: LOAD, table: "users" })
    ]);
    expect(v.phase.load).toBe("active");
    expect(v.resources.find((r) => r.table === "users")?.status).toBe("loading");

    const v2 = reduceAirwayEvents([
      loadStarted(),
      extract("users", 2),
      normalize("users", 2),
      destStart(["users"]),
      ev("table_load_started", { pipeline_name: PIPE, load_id: LOAD, table: "users" }),
      ev("table_loaded", { pipeline_name: PIPE, load_id: LOAD, table: "users", rows: 2 })
    ]);
    const users = v2.resources.find((r) => r.table === "users");
    expect(users?.status).toBe("done");
    expect(users?.rowsLoaded).toBe(2);
  });

  it("load_progress ticks rowsLoaded; table_load_failed → completed_with_errors", () => {
    const v = reduceAirwayEvents([
      loadStarted(),
      extract("orders", 50),
      normalize("orders", 50),
      ev("table_load_started", { pipeline_name: PIPE, load_id: LOAD, table: "orders" }),
      ev("load_progress", {
        pipeline_name: PIPE,
        load_id: LOAD,
        table: "orders",
        rows_written: 20
      }),
      ev("load_progress", {
        pipeline_name: PIPE,
        load_id: LOAD,
        table: "orders",
        rows_written: 50
      })
    ]);
    const orders = v.resources.find((r) => r.table === "orders");
    expect(orders?.status).toBe("loading");
    expect(orders?.rowsLoaded).toBe(50);
    expect(v.phase.load).toBe("active");

    const v2 = reduceAirwayEvents([
      loadStarted(),
      extract("orders", 1),
      normalize("orders", 1),
      ev("table_load_failed", {
        pipeline_name: PIPE,
        load_id: LOAD,
        table: "orders",
        error: "permission denied"
      }),
      loadCompleted({})
    ]);
    expect(v2.status).toBe("completed_with_errors");
    expect(v2.failedResources).toEqual([{ table: "orders", error: "permission denied" }]);
    expect(v2.resources.find((r) => r.table === "orders")?.status).toBe("error");
  });

  it("extract_progress ticks rowsExtracted while extracting", () => {
    const v = reduceAirwayEvents([
      loadStarted(),
      ev("extract_started", { pipeline_name: PIPE, load_id: LOAD, table: "orders" }),
      ev("extract_progress", {
        pipeline_name: PIPE,
        load_id: LOAD,
        table: "orders",
        rows_so_far: 100
      }),
      ev("extract_progress", {
        pipeline_name: PIPE,
        load_id: LOAD,
        table: "orders",
        rows_so_far: 350
      })
    ]);
    const orders = v.resources.find((r) => r.table === "orders");
    expect(orders?.status).toBe("extracting");
    expect(orders?.rowsExtracted).toBe(350);
    expect(v.phase.extract).toBe("active");
  });

  it("clean run (no resource_failed) stays done", () => {
    const v = reduceAirwayEvents([
      loadStarted(),
      extract("orders", 1),
      normalize("orders", 1),
      destStart(["orders"]),
      loadCompleted({ orders: 1 })
    ]);
    expect(v.status).toBe("done");
    expect(v.failedResources).toEqual([]);
  });

  it("cancelled marks the run cancelled without erroring rows", () => {
    const v = reduceAirwayEvents([
      loadStarted(),
      extract("users", 5),
      ev("cancelled", { pipeline_name: PIPE, load_id: LOAD })
    ]);
    expect(v.status).toBe("cancelled");
    expect(v.resources.find((r) => r.table === "users")?.status).not.toBe("error");
  });

  it("schema_evolved is surfaced for a badge", () => {
    const changes = [{ kind: "new_column", table: "users", column: "age" }];
    const v = reduceAirwayEvents([
      loadStarted(),
      ev("schema_evolved", { pipeline_name: PIPE, changes })
    ]);
    expect(v.schemaChanges).toEqual(changes);
  });

  it("streaming: child loaded BEFORE normalize_completed is still nested, not a root", () => {
    // Streaming order: a child's load events arrive before the
    // resource's normalize_completed (emitted at ResourceDone).
    // Nesting must come from the `__` name, not the late event.
    const tls = (table: string) =>
      ev("table_load_started", { pipeline_name: PIPE, load_id: LOAD, table });
    const v = reduceAirwayEvents([
      loadStarted(),
      extract("orders", 10),
      tls("orders"),
      tls("orders__checks"),
      tls("orders__checks__selections"),
      normalize("orders", 10, ["orders__checks", "orders__checks__selections"])
    ]);

    const byTable = (t: string) => v.resources.filter((r) => r.table === t);
    expect(byTable("orders__checks")).toHaveLength(1);
    expect(byTable("orders__checks__selections")).toHaveLength(1);
    expect(byTable("orders")[0].parent).toBeUndefined();
    expect(byTable("orders__checks")[0].parent).toBe("orders");
    expect(byTable("orders__checks__selections")[0].parent).toBe("orders__checks");
    expect(v.resources.filter((r) => !r.parent).map((r) => r.table)).toEqual(["orders"]);
  });

  it("deep descendants stay contiguous under their root even when another root interleaves", () => {
    // Repro of the reported bug: grandchildren (orders__checks__*)
    // were appended to the array end, landing visually under an
    // unrelated root (`jobs`) that arrived between the orders events.
    const tls = (table: string) =>
      ev("table_load_started", { pipeline_name: PIPE, load_id: LOAD, table });
    const v = reduceAirwayEvents([
      loadStarted(),
      extract("orders", 5000),
      tls("orders"),
      tls("orders__checks"),
      tls("orders__pricing_features"),
      // An unrelated root starts extracting in the middle of orders' load.
      ev("extract_started", { pipeline_name: PIPE, load_id: LOAD, table: "jobs" }),
      // Deep descendants arrive AFTER the `jobs` root row exists.
      tls("orders__checks__selections"),
      tls("orders__checks__selections__modifiers"),
      tls("orders__checks__payments")
    ]);

    const tables = v.resources.map((r) => r.table);
    const ordersIdx = tables.indexOf("orders");
    const jobsIdx = tables.indexOf("jobs");

    // Every orders-subtree row must sit after `orders` and BEFORE the
    // unrelated `jobs` root — i.e. the subtree is one contiguous block.
    for (const t of tables) {
      if (t === "orders" || t.startsWith("orders__")) {
        const i = tables.indexOf(t);
        expect(i).toBeGreaterThanOrEqual(ordersIdx);
        expect(i).toBeLessThan(jobsIdx);
      }
    }
    // Each descendant nests under its immediate parent (by `__`).
    const parentOf = (t: string) => v.resources.find((r) => r.table === t)?.parent;
    expect(parentOf("orders__checks")).toBe("orders");
    expect(parentOf("orders__checks__selections")).toBe("orders__checks");
    expect(parentOf("orders__checks__selections__modifiers")).toBe("orders__checks__selections");
    expect(parentOf("orders__checks__payments")).toBe("orders__checks");
    // No duplicates.
    expect(new Set(tables).size).toBe(tables.length);
  });

  it("captures per-phase timestamps from event `ts` and stays idempotent", () => {
    // The worker stamps every engine event's payload with `ts`; the reducer
    // records per-resource phase times for the run-timeline Gantt.
    const withTs = <E extends AirwayEvent>(e: E, ts: string): E => ({
      ...e,
      payload: { ...e.payload, ts }
    });

    const stream: AirwayEvent[] = [
      withTs(loadStarted(), "2026-05-17T00:00:00.000Z"),
      withTs(
        ev("extract_started", { pipeline_name: PIPE, load_id: LOAD, table: "orders" }),
        "2026-05-17T00:00:01.000Z"
      ),
      withTs(extract("orders", 10), "2026-05-17T00:00:05.000Z"),
      withTs(
        ev("normalize_started", { pipeline_name: PIPE, load_id: LOAD, table: "orders" }),
        "2026-05-17T00:00:05.000Z"
      ),
      withTs(normalize("orders", 10), "2026-05-17T00:00:06.000Z"),
      withTs(
        ev("table_load_started", { pipeline_name: PIPE, load_id: LOAD, table: "orders" }),
        "2026-05-17T00:00:06.000Z"
      ),
      withTs(
        ev("table_loaded", { pipeline_name: PIPE, load_id: LOAD, table: "orders", rows: 10 }),
        "2026-05-17T00:00:09.000Z"
      ),
      withTs(loadCompleted({ orders: 10 }), "2026-05-17T00:00:10.000Z")
    ];

    const v = reduceAirwayEvents(stream);
    expect(v.startedAt).toBe("2026-05-17T00:00:00.000Z");
    expect(v.endedAt).toBe("2026-05-17T00:00:10.000Z");
    const o = v.resources.find((r) => r.table === "orders");
    expect(o?.extractStartedAt).toBe("2026-05-17T00:00:01.000Z");
    expect(o?.extractEndedAt).toBe("2026-05-17T00:00:05.000Z");
    expect(o?.normalizeEndedAt).toBe("2026-05-17T00:00:06.000Z");
    expect(o?.loadStartedAt).toBe("2026-05-17T00:00:06.000Z");
    expect(o?.loadEndedAt).toBe("2026-05-17T00:00:09.000Z");

    // Pure/idempotent: re-reducing the same prefix yields the same view.
    expect(reduceAirwayEvents([...stream])).toEqual(v);
  });

  it("reads `ts` as a typed field of the payload", () => {
    // Part of every payload type, so a stamped event is a plain `AirwayEvent`.
    const cancelled: AirwayEvent = {
      type: "cancelled",
      payload: { pipeline_name: PIPE, load_id: LOAD, ts: "2026-05-17T00:00:11.000Z" }
    };
    expect(reduceAirwayEvents([loadStarted(), cancelled]).endedAt).toBe("2026-05-17T00:00:11.000Z");
  });
});
