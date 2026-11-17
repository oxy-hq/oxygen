import { describe, expect, it } from "vitest";
import type { RecentTraces } from "@/types/appRecentTraces";
import type { AppFunctionSummary } from "@/types/apps";
import { columnsFor, curve, layoutAppMap, NODE_HEIGHT, ROW } from "./appMapLayout";
import { buildAppMap } from "./appMapModel";

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

const traces: RecentTraces = {
  environment: "production",
  per_function: 5,
  retention_days: 3,
  functions: [
    {
      function: "sync",
      traces: [
        {
          invocation_id: "i1",
          created_at: "2026-11-17T10:00:00Z",
          mode: "schedule",
          failed: true,
          total_calls: 3,
          dropped: 0,
          touches: [
            { family: "query", op: "query", target: "orders", status: "ok", calls: 1 },
            {
              family: "query_stream",
              op: "query_stream",
              target: "orders",
              status: "ok",
              calls: 1
            },
            { family: "fetch", op: "fetch", target: "api.example.com", status: "error", calls: 1 }
          ]
        }
      ]
    }
  ]
};

const map = buildAppMap({
  functions: [
    fn("sync", { schedule: "0 * * * *", destinations: ["analytics"] }),
    fn("report", { route: false })
  ],
  traces
});

describe("layoutAppMap", () => {
  const layout = layoutAppMap(map, 800);

  it("splits the width into three columns that do not overlap", () => {
    const { entry, fn: fns, target } = columnsFor(800);
    expect(entry.x + entry.width).toBeLessThan(fns.x);
    expect(fns.x + fns.width).toBeLessThan(target.x);
    expect(target.x + target.width).toBe(800);
  });

  it("stacks a function's entry points in its band and centres the function on them", () => {
    const route = layout.boxes.get("entry:sync:route");
    const schedule = layout.boxes.get("entry:sync:schedule");
    const sync = layout.boxes.get("fn:sync");
    expect(route && schedule && sync).toBeTruthy();
    expect(schedule?.y).toBe((route?.y ?? 0) + ROW);
    expect(sync?.y).toBe((route?.y ?? 0) + ROW / 2);
    // The next function's band starts below this one's last entry.
    const manual = layout.boxes.get("entry:report:manual");
    expect(manual?.y).toBeGreaterThan((schedule?.y ?? 0) + NODE_HEIGHT);
  });

  it("puts every node in its column and every edge between two placed nodes", () => {
    for (const entry of map.entries) expect(layout.boxes.get(entry.id)?.x).toBe(0);
    for (const target of map.targets) {
      expect(layout.boxes.get(target.id)?.x).toBe(layout.columns.target.x);
    }
    expect([...layout.paths.keys()].sort()).toEqual(map.edges.map((e) => e.id).sort());
  });

  it("heads each target group once, above its first target", () => {
    expect(layout.headings.map((h) => h.label)).toEqual([
      "Warehouse writes",
      "Tables",
      "Outbound hosts"
    ]);
    const tables = layout.headings.find((h) => h.group === "tables");
    const orders = layout.boxes.get("target:tables:orders");
    expect((tables?.y ?? 0) < (orders?.y ?? 0)).toBe(true);
  });

  it("labels each target's edges by family, failed ones flagged, declared ones said", () => {
    const byTarget = Object.fromEntries(layout.labels.map((l) => [l.target, l]));
    expect(byTarget["target:tables:orders"]).toMatchObject({
      text: "query, query_stream",
      failed: false
    });
    expect(byTarget["target:fetch:api.example.com"]).toMatchObject({
      text: "fetch",
      failed: true
    });
    // The destination no recorded run wrote to.
    expect(byTarget["target:warehouse:analytics"].text).toBe("declared");
  });

  it("is tall enough for the longer side", () => {
    const bottoms = [...layout.boxes.values()].map((b) => b.y + b.height);
    expect(layout.height).toBeGreaterThanOrEqual(Math.max(...bottoms));
  });
});

describe("curve", () => {
  it("leaves and arrives level", () => {
    expect(curve({ x: 0, y: 10 }, { x: 100, y: 50 })).toBe("M 0 10 C 50 10, 50 50, 100 50");
  });
});
