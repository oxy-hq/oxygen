import { describe, expect, it } from "vitest";
import type { LocationRow } from "@/types/operatingGraph";
import {
  descendantIds,
  externalIdDiff,
  externalIdsProblem,
  locationPatch,
  locationSummary,
  locationTree,
  systemProblem,
  usedKinds
} from "./utils";

const location = (id: string, over: Partial<LocationRow> = {}): LocationRow => ({
  id,
  org_id: "org",
  name: id,
  kind: null,
  parent_id: null,
  status: "open",
  timezone: "UTC",
  external_id: null,
  external_ids: {},
  created_at: "2026-09-07T10:00:00Z",
  updated_at: "2026-09-07T10:00:00Z",
  ...over
});

const names = (rows: ReturnType<typeof locationTree>) =>
  rows.map((r) => `${"  ".repeat(r.depth)}${r.location.name}`);

describe("locationPatch", () => {
  const row = location("clovis", {
    name: "Clovis",
    kind: "store",
    status: "open",
    timezone: "America/Los_Angeles",
    external_id: "clovis"
  });
  const draft = {
    name: "Clovis",
    kind: "store",
    parentId: null,
    status: "open" as const,
    timezone: "America/Los_Angeles",
    externalId: "clovis"
  };

  it("sends nothing when the form matches the row, compared as the server stores", () => {
    expect(
      locationPatch(row, { ...draft, name: " Clovis ", kind: " Store ", externalId: " clovis " })
    ).toEqual({});
    expect(
      locationPatch(location("new", { external_id: null }), {
        ...draft,
        name: "new",
        kind: "",
        timezone: "UTC",
        externalId: ""
      })
    ).toEqual({});
  });

  it("sends the tenant's own id when it changes, and null to clear it", () => {
    expect(locationPatch(row, { ...draft, externalId: "santa-clara" })).toEqual({
      external_id: "santa-clara"
    });
    expect(locationPatch(row, { ...draft, externalId: "   " })).toEqual({ external_id: null });
  });

  it("sends only the fields that differ", () => {
    expect(
      locationPatch(row, { ...draft, kind: "Region", parentId: "west", status: "archived" })
    ).toEqual({ kind: "region", parent_id: "west", status: "archived" });
  });
});

describe("locationTree", () => {
  it("lists roots first, each followed by its children, siblings by name", () => {
    const rows = locationTree([
      location("store-b", { parent_id: "west" }),
      location("east"),
      location("west"),
      location("store-a", { parent_id: "west" }),
      location("station", { parent_id: "store-a" })
    ]);
    expect(names(rows)).toEqual(["east", "west", "  store-a", "    station", "  store-b"]);
  });

  it("renders a location whose parent is missing as a root", () => {
    const rows = locationTree([location("orphan", { parent_id: "gone" }), location("root")]);
    expect(names(rows)).toEqual(["orphan", "root"]);
    expect(rows.every((r) => r.depth === 0)).toBe(true);
  });

  it("terminates on a cycle and still shows every location", () => {
    const rows = locationTree([
      location("a", { parent_id: "b" }),
      location("b", { parent_id: "a" }),
      location("root")
    ]);
    expect(rows.map((r) => r.location.id).sort()).toEqual(["a", "b", "root"]);
  });
});

describe("descendantIds", () => {
  const tree = [
    location("region"),
    location("store", { parent_id: "region" }),
    location("station", { parent_id: "store" }),
    location("other", { parent_id: "region" })
  ];

  it("is the location and everything under it — what the parent picker must not offer", () => {
    expect([...descendantIds(tree, "region")].sort()).toEqual(
      ["other", "region", "station", "store"].sort()
    );
    expect([...descendantIds(tree, "store")].sort()).toEqual(["station", "store"]);
    expect([...descendantIds(tree, "station")]).toEqual(["station"]);
  });

  it("excludes ancestors and siblings, so re-parenting sideways stays possible", () => {
    const excluded = descendantIds(tree, "store");
    expect(excluded.has("region")).toBe(false);
    expect(excluded.has("other")).toBe(false);
  });

  it("terminates on a cycle", () => {
    const cyclic = [location("a", { parent_id: "b" }), location("b", { parent_id: "a" })];
    expect([...descendantIds(cyclic, "a")].sort()).toEqual(["a", "b"]);
  });
});

describe("systemProblem", () => {
  it("accepts lowercase tokens up to 32 chars", () => {
    expect(systemProblem("toast")).toBeNull();
    expect(systemProblem("unifi-site_2")).toBeNull();
    expect(systemProblem("a".repeat(32))).toBeNull();
  });

  it("names the rule for anything else", () => {
    for (const bad of ["", "Toast", "toast pos", "toast.pos", "a".repeat(33)]) {
      expect(systemProblem(bad)).toMatch(/lowercase/);
    }
  });
});

describe("externalIdsProblem", () => {
  it("passes a clean set", () => {
    expect(
      externalIdsProblem([
        { key: 0, system: "toast", id: "1234" },
        { key: 1, system: "unifi", id: "site-9" }
      ])
    ).toBeNull();
    expect(externalIdsProblem([])).toBeNull();
  });

  it("refuses a bad system, a missing id, or the same system twice", () => {
    expect(externalIdsProblem([{ key: 0, system: "Toast", id: "1" }])).toMatch(/lowercase/);
    expect(externalIdsProblem([{ key: 0, system: "toast", id: " " }])).toMatch(/Enter the id/);
    expect(
      externalIdsProblem([
        { key: 0, system: "toast", id: "1" },
        { key: 1, system: "toast", id: "2" }
      ])
    ).toMatch(/listed twice/);
  });
});

describe("externalIdDiff", () => {
  it("puts what was added or changed and deletes what went", () => {
    const diff = externalIdDiff(
      { toast: "1", unifi: "old", payroll: "p" },
      { toast: "1", unifi: "new", momos: "m" }
    );
    expect(diff.set.sort()).toEqual([
      ["momos", "m"],
      ["unifi", "new"]
    ]);
    expect(diff.remove).toEqual(["payroll"]);
  });

  it("is empty when nothing changed", () => {
    expect(externalIdDiff({ toast: "1" }, { toast: "1" })).toEqual({ set: [], remove: [] });
  });
});

describe("usedKinds and locationSummary", () => {
  it("lists distinct kinds by name and counts open locations", () => {
    const rows = [
      location("a", { kind: "store" }),
      location("b", { kind: "region", status: "pre_launch" }),
      location("c", { kind: "store", status: "archived" }),
      location("d")
    ];
    expect(usedKinds(rows)).toEqual(["region", "store"]);
    expect(locationSummary(rows)).toBe("4 locations · 2 open");
    expect(locationSummary([location("a")])).toBe("1 location · 1 open");
  });
});
