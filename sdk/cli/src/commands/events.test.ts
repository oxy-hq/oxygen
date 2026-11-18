/**
 * `oxyc events` — the engineer's read and redelivery surface over the app
 * events routes, driven with `globalThis.fetch` stubbed.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { type CliError, ExitCode } from "../util/errors.js";
import {
  runEnvEvents,
  runEventsDead,
  runEventsFind,
  runEventsLs,
  runEventsRedeliver,
  runEventsShow
} from "./events.js";
import { runEventsResync } from "./events-resync.js";

const TARGET = "https://oxy.test";

function fakeContext(): Context {
  const ctx: Context = {
    cwd: "/tmp",
    flags: { env: "production", apiKeyEnv: "OXY_API_KEY", org: "acme" },
    target: () => TARGET,
    env: () => ({ target: TARGET, orgSlug: undefined }) as ReturnType<Context["env"]>,
    bearer: async () => "tok",
    maybeBearer: async () => "tok",
    storedBearer: () => "tok",
    async credential() {
      return { token: "tok", source: "env" };
    },
    serviceAccount: () => undefined,
    apiKey: () => undefined,
    customer: () => undefined,
    repoDir: () => undefined,
    placeholders: () => ({}),
    withEnv: () => ctx
  };
  return ctx;
}

interface Call {
  method: string;
  path: string;
  body?: unknown;
}

function stubFetch(routes: Record<string, unknown>, calls: Call[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | URL, init?: RequestInit) => {
      const path = String(input).replace(TARGET, "");
      const method = (init?.method ?? "GET").toUpperCase();
      calls.push({
        method,
        path,
        body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined
      });
      const key = `${method} ${path}`;
      if (!(key in routes)) {
        return new Response(JSON.stringify({ error: `no stub for ${key}` }), { status: 404 });
      }
      return new Response(JSON.stringify(routes[key]), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    })
  );
}

function capture() {
  let printed = "";
  const write = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
    printed += String(chunk);
    return true;
  });
  return {
    text: () => printed,
    restore: () => write.mockRestore()
  };
}

const listener = (over: Record<string, unknown>) => ({
  listener: "bookkeeping/on-statement",
  environment: "production",
  pending: 0,
  retrying: 0,
  dead: 0,
  held: 0,
  oldestPendingAgeSeconds: null,
  lastSuccessAt: null,
  lastFailureAt: null,
  lastFailureOutcome: null,
  inFlight: 0,
  buildId: "b-1",
  reconciledAt: "2026-11-17T10:00:00Z",
  ...over
});

const TYPES = {
  types: [
    {
      type: "locations/statement.uploaded",
      publisher: "locations",
      schemaRevision: 3,
      lastPublishedAt: "2026-11-17T09:00:00Z",
      listeners: [
        listener({ listener: "bookkeeping/fresh", oldestPendingAgeSeconds: 5 }),
        listener({ listener: "bookkeeping/stuck", pending: 4, oldestPendingAgeSeconds: 900 }),
        listener({ listener: "bookkeeping/idle" })
      ]
    }
  ]
};

describe("oxyc events", () => {
  let calls: Call[];
  beforeEach(() => {
    calls = [];
  });
  afterEach(() => vi.unstubAllGlobals());

  it("events ls prints a table, and --json prints the response", async () => {
    stubFetch({ "GET /api/customer-apps/acme/events/types": TYPES }, calls);
    const table = capture();
    await runEventsLs(fakeContext(), { json: false });
    table.restore();
    expect(table.text()).toContain("locations/statement.uploaded");
    expect(table.text()).toContain("bookkeeping/stuck");
    expect(table.text()).toContain("b-1");

    const json = capture();
    await runEventsLs(fakeContext(), { json: true });
    json.restore();
    expect(JSON.parse(json.text())).toEqual(TYPES);
  });

  it("events ls sorts listeners by oldest pending age", async () => {
    stubFetch({ "GET /api/customer-apps/acme/events/types": TYPES }, calls);
    const table = capture();
    await runEventsLs(fakeContext(), { json: false });
    table.restore();
    const at = (name: string) => table.text().indexOf(name);
    expect(at("bookkeeping/stuck")).toBeGreaterThan(-1);
    expect(at("bookkeeping/stuck")).toBeLessThan(at("bookkeeping/fresh"));
    expect(at("bookkeeping/fresh")).toBeLessThan(at("bookkeeping/idle"));
  });

  it("events ls lists table subscriptions with their cursor and why one needs a person", async () => {
    const tables = [
      {
        table: "pokehouse:bookkeeping.lines",
        type: "table:pokehouse:bookkeeping.lines",
        environment: "production",
        app: "locations",
        function: "on-lines",
        state: "active",
        detail: null,
        snapshot: 40,
        head: 42,
        lag: 2,
        checkedAt: "2026-11-18T10:00:00Z",
        advancedAt: "2026-11-18T09:59:00Z",
        pausedAt: null
      },
      {
        table: "app_locations.visits",
        type: "table:app_locations.visits",
        environment: "production",
        app: "locations",
        function: "on-visits",
        state: "behind_horizon",
        detail: "the cursor is at snapshot 3 and snapshot 9 is the oldest Airhouse still holds",
        snapshot: 3,
        head: 90,
        lag: 87,
        checkedAt: null,
        advancedAt: null,
        pausedAt: null
      }
    ];
    stubFetch({ "GET /api/customer-apps/acme/events/types": { ...TYPES, tables } }, calls);
    const out = capture();
    await runEventsLs(fakeContext(), { json: false });
    out.restore();
    expect(out.text()).toContain("Table subscriptions");
    expect(out.text()).toMatch(
      /pokehouse:bookkeeping\.lines\s+locations\/on-lines\s+production\s+active\s+40\s+42\s+2/
    );
    expect(out.text()).toContain("BEHIND HORIZON");
    expect(out.text()).toContain("oldest Airhouse still holds");

    const json = capture();
    await runEventsLs(fakeContext(), { json: true });
    json.restore();
    expect(JSON.parse(json.text()).tables).toHaveLength(2);
  });

  it("events ls without table subscriptions prints no such section (an older server's reply too)", async () => {
    stubFetch({ "GET /api/customer-apps/acme/events/types": TYPES }, calls);
    const out = capture();
    await runEventsLs(fakeContext(), { json: false });
    out.restore();
    expect(out.text()).not.toContain("Table subscriptions");
  });

  it("events resync posts the function and environment and reports where each cursor moved", async () => {
    stubFetch(
      {
        "POST /api/customer-apps/acme/locations/events/resync": {
          function: "on-lines",
          environment: "production",
          tables: [
            { table: "pokehouse:bookkeeping.lines", from: 3, to: 90, state: "behind_horizon" }
          ],
          backfill: true,
          queued: 1,
          dryRun: false
        }
      },
      calls
    );
    const out = capture();
    await runEventsResync(fakeContext(), "acme/locations", "on-lines", {
      backfill: true,
      json: false
    });
    out.restore();
    expect(calls[0]?.body).toEqual({
      function: "on-lines",
      environment: "production",
      backfill: true
    });
    expect(out.text()).toContain(
      "moved pokehouse:bookkeeping.lines: snapshot 3 -> 90 (was behind_horizon)"
    );
    expect(out.text()).toContain("backfill: 1 run(s) queued");
  });

  it("events resync --dry-run changes nothing and says so", async () => {
    stubFetch(
      {
        "POST /api/customer-apps/acme/locations/events/resync": {
          function: "on-lines",
          environment: "staging",
          tables: [{ table: "app_locations.visits", from: 5, to: 8, state: "active" }],
          backfill: false,
          queued: 0,
          dryRun: true
        }
      },
      calls
    );
    const out = capture();
    await runEventsResync(fakeContext(), "acme/locations", "on-lines", {
      appEnv: "staging",
      dryRun: true,
      json: false
    });
    out.restore();
    expect(calls[0]?.body).toEqual({ function: "on-lines", environment: "staging", dryRun: true });
    expect(out.text()).toContain("would move app_locations.visits");
    expect(out.text()).toContain("dry run: nothing changed");
  });

  it("events show reads one event, and --chain reads every event of its root", async () => {
    stubFetch(
      {
        "GET /api/customer-apps/acme/events/ev-1": { id: "ev-1", rootId: "root-9", deliveries: [] },
        "GET /api/customer-apps/acme/events?root=root-9": {
          events: [{ id: "root-9" }, { id: "ev-1" }]
        }
      },
      calls
    );
    const show = capture();
    await runEventsShow(fakeContext(), "ev-1", { chain: false, json: true });
    show.restore();
    expect(JSON.parse(show.text()).id).toBe("ev-1");

    const chain = capture();
    await runEventsShow(fakeContext(), "ev-1", { chain: true, json: true });
    chain.restore();
    expect(calls.map((c) => c.path)).toContain("/api/customer-apps/acme/events?root=root-9");
  });

  it("events find calls the find route with type and key", async () => {
    const path =
      "/api/customer-apps/acme/events/find?type=locations%2Fstatement.uploaded&key=statement%3A123&environment=staging";
    stubFetch({ [`GET ${path}`]: { id: "ev-7", deliveries: [] } }, calls);
    const out = capture();
    await runEventsFind(fakeContext(), "locations/statement.uploaded", {
      key: "statement:123",
      appEnv: "staging",
      json: true
    });
    out.restore();
    expect(calls[0]?.path).toBe(path);
    expect(JSON.parse(out.text()).id).toBe("ev-7");
  });

  it("events dead prints one block per error signature", async () => {
    stubFetch(
      {
        "GET /api/customer-apps/acme/bookkeeping/events/dead?environment=production": {
          groups: [
            {
              signature: "error: ledger closed",
              count: 2,
              deliveries: [
                {
                  eventId: "ev-1",
                  listener: "bookkeeping/on-statement",
                  attempts: [{ n: 1, outcome: "error", error: "ledger closed" }]
                },
                { eventId: "ev-2", listener: "bookkeeping/on-statement", attempts: [] }
              ]
            },
            { signature: "timeout: ", count: 1, deliveries: [{ eventId: "ev-3", attempts: [] }] }
          ]
        }
      },
      calls
    );
    const out = capture();
    await runEventsDead(fakeContext(), "acme/bookkeeping", { json: false });
    out.restore();
    const blocks = out.text().split(/\n\n+/).filter(Boolean);
    expect(blocks).toHaveLength(2);
    expect(blocks[0]).toContain("error: ledger closed");
    expect(blocks[0]).toContain("ev-1");
    expect(blocks[0]).toContain("ev-2");
    expect(blocks[1]).toContain("timeout");
  });

  it("redeliver --dry-run prints the count and sends dryRun", async () => {
    stubFetch({ "POST /api/customer-apps/acme/events/redeliver": { count: 12 } }, calls);
    const out = capture();
    await runEventsRedeliver(fakeContext(), undefined, {
      to: "bookkeeping/on-statement",
      status: "dead",
      dryRun: true,
      json: false
    });
    out.restore();
    expect(calls[0]?.body).toEqual({
      to: "bookkeeping/on-statement",
      status: "dead",
      dryRun: true
    });
    expect(out.text()).toContain("12");
  });

  it("redeliver of one event posts to its route", async () => {
    stubFetch({ "POST /api/customer-apps/acme/events/ev-1/redeliver": { count: 1 } }, calls);
    const out = capture();
    await runEventsRedeliver(fakeContext(), "ev-1", { to: "bookkeeping/fn", json: true });
    out.restore();
    expect(calls[0]?.body).toEqual({ to: "bookkeeping/fn" });
  });

  it("env events flips the switch with PUT {enabled}", async () => {
    stubFetch(
      {
        "PUT /api/customer-apps/acme/store/environments/staging/events": {
          environment: "staging",
          enabled: true
        }
      },
      calls
    );
    const out = capture();
    await runEnvEvents(fakeContext(), "acme/store", { appEnv: "staging", on: true, json: true });
    out.restore();
    expect(calls[0]?.body).toEqual({ enabled: true });
  });

  it("env events refuses --on with --off, before any request", async () => {
    stubFetch({}, calls);
    const err = await runEnvEvents(fakeContext(), "acme/store", {
      appEnv: "staging",
      on: true,
      off: true,
      json: true
    }).catch((e: unknown) => e as CliError);
    expect((err as CliError).code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
  });
});
