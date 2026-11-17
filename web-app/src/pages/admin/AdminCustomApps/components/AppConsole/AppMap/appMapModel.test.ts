import { describe, expect, it } from "vitest";
import type { RecentTraces, TraceDigest, TraceTouch } from "@/types/appRecentTraces";
import type { AppFunctionSummary, AppSecretEntry } from "@/types/apps";
import { buildAppMap, entriesOf, functionId, placementOf } from "./appMapModel";

const fn = (name: string, over: Partial<AppFunctionSummary> = {}): AppFunctionSummary => ({
  name,
  route: true,
  schedule: null,
  timezone: null,
  airway: false,
  timeout_seconds: null,
  retries: null,
  secrets_write: false,
  destinations: [],
  ...over
});

const touch = (family: string, target?: string, over: Partial<TraceTouch> = {}): TraceTouch => ({
  family,
  op: family,
  target,
  status: "ok",
  calls: 1,
  ...over
});

let seq = 0;
const run = (at: string, touches: TraceTouch[], failed = false): TraceDigest => ({
  invocation_id: `inv-${seq++}`,
  created_at: at,
  mode: "route",
  failed,
  total_calls: touches.reduce((n, t) => n + t.calls, 0),
  dropped: 0,
  touches
});

const traces = (functions: Record<string, TraceDigest[]>): RecentTraces => ({
  environment: "production",
  per_function: 5,
  retention_days: 3,
  functions: Object.entries(functions).map(([name, runs]) => ({ function: name, traces: runs }))
});

const secret = (key: string, over: Partial<AppSecretEntry> = {}): AppSecretEntry => ({
  key,
  is_set: true,
  declared: true,
  required: false,
  source: "manifest",
  ...over
});

describe("entriesOf", () => {
  it("lists every way a function is started, in a fixed order", () => {
    const entries = entriesOf(
      fn("sync", {
        schedule: "0 * * * *",
        timezone: "Europe/London",
        airway: true,
        check: true,
        webhook: { signature_header: "stripe-signature", secret_var: "STRIPE_SECRET" }
      })
    );
    expect(entries.map((e) => [e.kind, e.detail])).toEqual([
      ["route", "fn/sync"],
      ["webhook", "stripe-signature"],
      ["schedule", "0 * * * * · Europe/London"],
      ["airway", null],
      ["check", null]
    ]);
  });

  it("says when nothing in the manifest starts a function", () => {
    const entries = entriesOf(fn("backfill", { route: false }));
    expect(entries.map((e) => [e.kind, e.label])).toEqual([["manual", "Run now only"]]);
  });
});

describe("placementOf", () => {
  it("puts a warehouse write on its database, with the table as detail", () => {
    expect(placementOf({ family: "warehouse", target: "analytics.orders" })).toMatchObject({
      group: "warehouse",
      key: "analytics",
      label: "analytics",
      detail: "orders"
    });
    expect(placementOf({ family: "warehouse", target: "analytics" }).detail).toBeNull();
  });

  it("groups by family and keeps a call whose target was not recorded", () => {
    expect(placementOf({ family: "query_stream", target: "events", verb: "SELECT" })).toMatchObject(
      { group: "tables", key: "events", detail: "SELECT" }
    );
    expect(placementOf({ family: "tx", target: "app.orders" }).group).toBe("oltp");
    expect(placementOf({ family: "fetch" })).toMatchObject({
      group: "fetch",
      label: "Host not recorded"
    });
    expect(placementOf({ family: "notify" })).toMatchObject({
      group: "messaging",
      label: "Notifications"
    });
    expect(placementOf({ family: "email" }).label).toBe("Email");
    expect(placementOf({ family: "secrets", target: "OAUTH_TOKEN" }).group).toBe("secrets");
  });

  it("does not drop a family it has never heard of", () => {
    expect(placementOf({ family: "teleport", target: "mars" })).toMatchObject({
      group: "other",
      label: "teleport · mars"
    });
  });
});

describe("buildAppMap", () => {
  it("draws the manifest before any trace is read, and says the right side is incomplete", () => {
    const map = buildAppMap({
      functions: [fn("sync", { schedule: "*/5 * * * *", destinations: ["analytics"] })]
    });
    expect(map.freshness).toBeNull();
    expect(map.functions).toEqual([{ id: "fn:sync", name: "sync", runs: null, failedRuns: 0 }]);
    expect(map.targets).toEqual([
      expect.objectContaining({
        id: "target:warehouse:analytics",
        declared: true,
        observed: false,
        calls: 0
      })
    ]);
    const kinds = map.edges.map((e) => [e.kind, e.from, e.to]);
    expect(kinds).toEqual([
      ["trigger", "entry:sync:route", "fn:sync"],
      ["trigger", "entry:sync:schedule", "fn:sync"],
      ["touch", "fn:sync", "target:warehouse:analytics"]
    ]);
  });

  it("aggregates every run of every function onto shared targets", () => {
    const map = buildAppMap({
      functions: [fn("ingest", { destinations: ["analytics"] }), fn("report")],
      traces: traces({
        ingest: [
          run("2026-11-17T10:00:00Z", [
            touch("warehouse", "analytics.orders", { calls: 3 }),
            touch("fetch", "api.stripe.com", { verb: "GET", calls: 2 })
          ]),
          run("2026-11-17T09:00:00Z", [touch("warehouse", "analytics.refunds")])
        ],
        report: [run("2026-11-17T08:00:00Z", [touch("query", "orders", { verb: "SELECT" })])]
      })
    });

    const analytics = map.targets.find((t) => t.id === "target:warehouse:analytics");
    expect(analytics).toMatchObject({
      declared: true,
      observed: true,
      calls: 4,
      details: ["orders", "refunds"]
    });
    const edge = map.edges.find((e) => e.id === "fn:ingest->target:warehouse:analytics");
    expect(edge).toMatchObject({ declared: true, observed: true, families: ["warehouse"] });
    expect(map.targets.find((t) => t.group === "fetch")).toMatchObject({
      label: "api.stripe.com",
      calls: 2,
      details: ["GET"]
    });
    expect(map.functions.map((f) => [f.name, f.runs])).toEqual([
      ["ingest", 2],
      ["report", 1]
    ]);
  });

  it("marks a target failed only by calls that ended in error", () => {
    const map = buildAppMap({
      functions: [fn("sync"), fn("other")],
      traces: traces({
        sync: [
          run("2026-11-17T10:00:00Z", [
            touch("fetch", "api.vendor.com", { status: "error", error_type: "timeout", calls: 2 }),
            touch("fetch", "api.vendor.com", { calls: 5 }),
            // Held and refused are a non-production environment's policy, not a failure.
            touch("email", undefined, { status: "held" }),
            touch("fetch", "hooks.slack.com", { status: "refused" })
          ])
        ],
        other: [run("2026-11-17T09:00:00Z", [touch("fetch", "api.vendor.com")])]
      })
    });
    const vendor = map.targets.find((t) => t.label === "api.vendor.com");
    expect(vendor).toMatchObject({ calls: 8, failedCalls: 2 });
    const failedEdges = map.edges.filter((e) => e.failedCalls > 0).map((e) => e.id);
    // Only the function whose calls failed has a failed edge into the shared host.
    expect(failedEdges).toEqual(["fn:sync->target:fetch:api.vendor.com"]);
    expect(map.targets.filter((t) => t.failedCalls > 0).map((t) => t.label)).toEqual([
      "api.vendor.com"
    ]);
  });

  it("states the window it drew from, and which functions it has nothing on", () => {
    const map = buildAppMap({
      functions: [fn("a"), fn("b"), fn("c")],
      traces: traces({
        a: [run("2026-11-17T10:00:00Z", []), run("2026-11-15T10:00:00Z", [], true)],
        b: [],
        // Not in this build any more: its runs say nothing about the build mapped.
        retired: [run("2026-11-17T12:00:00Z", [touch("fetch", "old.example.com")])]
      })
    });
    expect(map.freshness).toEqual({
      perFunction: 5,
      runs: 2,
      oldest: "2026-11-15T10:00:00Z",
      newest: "2026-11-17T10:00:00Z",
      retentionDays: 3,
      // `c` was not answered for at all: as quiet as `b`.
      quiet: ["b", "c"]
    });
    expect(map.targets).toEqual([]);
    expect(map.functions.find((f) => f.name === "a")).toMatchObject({ runs: 2, failedRuns: 1 });
  });

  it("orders each group's targets beside the functions that reach them", () => {
    const map = buildAppMap({
      functions: [fn("alpha"), fn("beta"), fn("gamma")],
      traces: traces({
        alpha: [run("2026-11-17T10:00:00Z", [touch("query", "zebra")])],
        beta: [run("2026-11-17T10:00:00Z", [touch("fetch", "x.example.com")])],
        gamma: [run("2026-11-17T10:00:00Z", [touch("query", "aardvark")])]
      })
    });
    // Tables before hosts (group order); within tables, alpha's before gamma's
    // even though "aardvark" sorts first by name.
    expect(map.targets.map((t) => t.label)).toEqual(["zebra", "aardvark", "x.example.com"]);
  });

  it("names declared secrets no recorded call names, but not a webhook's signing key", () => {
    const map = buildAppMap({
      functions: [fn("sync")],
      traces: traces({
        sync: [
          run("2026-11-17T10:00:00Z", [touch("secrets", "OAUTH_TOKEN", { op: "secrets.set" })])
        ]
      }),
      secrets: [
        secret("STRIPE_KEY"),
        secret("OAUTH_TOKEN"),
        secret("WEBHOOK_SECRET", { source: "webhook" }),
        secret("LEFTOVER", { declared: false, source: "undeclared" }),
        secret("API_BASE")
      ]
    });
    expect(map.untracedSecrets).toEqual(["API_BASE", "STRIPE_KEY"]);
  });

  it("ignores a function's traces when that function is not in the build", () => {
    const map = buildAppMap({
      functions: [fn("kept")],
      traces: traces({ gone: [run("2026-11-17T10:00:00Z", [touch("fetch", "x.example.com")])] })
    });
    expect(map.edges.some((e) => e.from === functionId("gone"))).toBe(false);
    expect(map.functions.map((f) => f.runs)).toEqual([0]);
  });
});
