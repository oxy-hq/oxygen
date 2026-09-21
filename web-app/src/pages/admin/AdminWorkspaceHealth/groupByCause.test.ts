import { describe, expect, it } from "vitest";
import type {
  WorkspaceHealthDimension,
  WorkspaceHealthEntry,
  WorkspaceHealthStatus
} from "@/services/api/workspaceHealth";
import { groupByCause, normalizeReason, worseStatus } from "./groupByCause";

/**
 * The seeded fleet is the motivating case and the first test: nine workspaces, nine
 * copies of one 206-character string. If this file ever stops asserting that collapses
 * to a single cause, the page has regressed to the wall of duplicates it replaced.
 */

const entry = (
  name: string,
  status: WorkspaceHealthStatus,
  reasons: string[],
  dimensions: WorkspaceHealthDimension[] = []
): WorkspaceHealthEntry => ({
  workspace_id: `id-${name}`,
  workspace_name: name,
  org_name: `${name} Inc`,
  status,
  reasons,
  dimensions,
  signals: null,
  reconciliation: [],
  smoke: [],
  smoke_probes: [],
  last_smoke_at: null,
  changed_at: null,
  checked_at: null
});

const SEEDED =
  "semantic:restaurant_ops: measures [toast_sales.net_sales] failed: metric-tree op failed: Query error: could not build connector config for database 'oltp' (type postgres_managed)";

describe("normalizeReason", () => {
  it("drops the volatile probe-count tail the backend appends", () => {
    expect(normalizeReason(`${SEEDED} (+22 more failing probe(s))`)).toBe(SEEDED);
    expect(normalizeReason(`${SEEDED} (+1 more failing probe(s))`)).toBe(SEEDED);
  });

  it("keeps everything that actually distinguishes two incidents", () => {
    expect(normalizeReason("connection:postgres refused on port 5445")).toBe(
      "connection:postgres refused on port 5445"
    );
  });
});

describe("worseStatus", () => {
  it("ranks unhealthy over degraded over healthy, either way round", () => {
    expect(worseStatus("degraded", "unhealthy")).toBe("unhealthy");
    expect(worseStatus("unhealthy", "degraded")).toBe("unhealthy");
    expect(worseStatus("healthy", "degraded")).toBe("degraded");
  });
});

describe("groupByCause", () => {
  it("collapses the seeded fleet's nine identical failures into one cause", () => {
    const fleet = ["Globex", "Vandelay", "Initech", "Acme", "Northwind", "Demo", "Umbrella"].map(
      (n) => entry(n, "unhealthy", [SEEDED])
    );
    const causes = groupByCause(fleet);
    expect(causes).toHaveLength(1);
    expect(causes[0].workspaces).toHaveLength(7);
    expect(causes[0].reason).toBe(SEEDED);
    expect(causes[0].status).toBe("unhealthy");
  });

  it("groups across differing probe-count tails, which are the same incident", () => {
    const causes = groupByCause([
      entry("a", "unhealthy", [`${SEEDED} (+22 more failing probe(s))`]),
      entry("b", "unhealthy", [`${SEEDED} (+3 more failing probe(s))`]),
      entry("c", "unhealthy", [SEEDED])
    ]);
    expect(causes).toHaveLength(1);
    expect(causes[0].workspaces.map((w) => w.workspace_name)).toEqual(["a", "b", "c"]);
  });

  it("keeps genuinely different failures apart", () => {
    const causes = groupByCause([
      entry("a", "unhealthy", ["connection:postgres refused"]),
      entry("b", "unhealthy", ["connection:clickhouse timed out"])
    ]);
    expect(causes).toHaveLength(2);
  });

  it("lists a workspace under each of its several reasons", () => {
    const causes = groupByCause([entry("a", "unhealthy", ["cause one", "cause two"])]);
    expect(causes).toHaveLength(2);
    expect(causes.every((c) => c.workspaces[0].workspace_name === "a")).toBe(true);
  });

  it("does not double-count a reason a workspace reports twice", () => {
    const causes = groupByCause([entry("a", "unhealthy", ["same", "same"])]);
    expect(causes).toHaveLength(1);
    expect(causes[0].workspaces).toHaveLength(1);
  });

  it("sorts worst first, then by how many workspaces a cause takes down", () => {
    const causes = groupByCause([
      entry("a", "degraded", ["wide but only degraded"]),
      entry("b", "degraded", ["wide but only degraded"]),
      entry("c", "degraded", ["wide but only degraded"]),
      entry("d", "unhealthy", ["narrow but down"]),
      entry("e", "unhealthy", ["two are down"]),
      entry("f", "unhealthy", ["two are down"])
    ]);
    expect(causes.map((c) => c.reason)).toEqual([
      "two are down",
      "narrow but down",
      "wide but only degraded"
    ]);
  });

  it("takes the worst status among the workspaces sharing a cause", () => {
    const causes = groupByCause([
      entry("a", "degraded", ["shared"]),
      entry("b", "unhealthy", ["shared"])
    ]);
    expect(causes[0].status).toBe("unhealthy");
  });

  it("reports the dimension a cause came from, without repeating it", () => {
    const dim: WorkspaceHealthDimension = {
      dimension: "smoke_test",
      status: "unhealthy",
      reason: `${SEEDED} (+22 more failing probe(s))`
    };
    const causes = groupByCause([
      entry("a", "unhealthy", [SEEDED], [dim]),
      entry("b", "unhealthy", [SEEDED], [dim])
    ]);
    expect(causes[0].dimensions).toEqual(["smoke_test"]);
  });

  it("leaves healthy workspaces out of the causes entirely", () => {
    const causes = groupByCause([
      entry("fine", "healthy", []),
      entry("broken", "unhealthy", ["x"])
    ]);
    expect(causes).toHaveLength(1);
    expect(causes[0].reason).toBe("x");
  });

  it("surfaces an unhealthy workspace that reported no reason, rather than dropping it", () => {
    const causes = groupByCause([entry("mystery", "unhealthy", [])]);
    expect(causes).toHaveLength(1);
    expect(causes[0].workspaces[0].workspace_name).toBe("mystery");
  });

  it("returns nothing for an all-healthy fleet", () => {
    expect(groupByCause([entry("a", "healthy", []), entry("b", "healthy", [])])).toEqual([]);
  });
});
