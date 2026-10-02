// The decision table, one row per shape of caller. What this pins is the
// ORDER the shapes are decided in and the fail-closed answers — the part a
// reader would get wrong from the rule alone.
import { describe, expect, it } from "vitest";
import {
  adminOnly,
  isAdmin,
  predicate,
  type Reach,
  type ReachCtx,
  reaches,
  reachOf,
  requireReach
} from "./index";

const ctx = (user: ReachCtx["user"]): ReachCtx => ({ user });
const scoped = (...locations: string[]): Reach => ({ everywhere: false, via: null, locations });

describe("reachOf", () => {
  it("hands back what the platform decided, defensively copied", () => {
    const platform: Reach = { everywhere: false, via: null, locations: ["clovis", "fresno"] };
    const r = reachOf(ctx({ reach: platform }));
    expect(r).toEqual(platform);
    r.locations.push("santa-rosa");
    expect(platform.locations).toHaveLength(2);
  });

  it("carries every reason the platform states", () => {
    for (const via of ["system", "app-admin", "org-wide-position", "org-member"] as const) {
      expect(reachOf(ctx({ reach: { everywhere: true, via, locations: [] } })).via).toBe(via);
    }
  });

  it("lands on nowhere when the server sent no reach — never on the office", () => {
    const nowhere = { everywhere: false, via: null, locations: [] };
    expect(reachOf(ctx({}))).toEqual(nowhere);
    expect(reachOf(ctx({ reach: null }))).toEqual(nowhere);
    // A system invocation on an older server is not an exception: the
    // module must not invent a wider answer than the platform gave.
    expect(reachOf(ctx({ kind: "system", appRole: "admin" }))).toEqual(nowhere);
    // Nor does a malformed shape widen anything.
    expect(reachOf(ctx({ reach: { everywhere: true } as unknown as Reach }))).toEqual(nowhere);
  });

  it("can be awaited by code written against the app-side version", async () => {
    // oxlint-disable-next-line typescript/await-thenable -- awaiting the synchronous return is what this test is about
    expect(await reachOf(ctx({ reach: scoped("clovis") }))).toEqual(scoped("clovis"));
  });
});

describe("reaches / requireReach", () => {
  it("is in reach when everywhere or listed", () => {
    expect(reaches(scoped("clovis"), "clovis")).toBe(true);
    expect(reaches(scoped("clovis"), "fresno")).toBe(false);
    expect(reaches({ everywhere: true, via: "org-member", locations: [] }, "fresno")).toBe(true);
  });

  it("is null in reach and a 403 with a stable code out of it", async () => {
    expect(requireReach(ctx({ reach: scoped("clovis") }), "clovis")).toBeNull();
    const refused = requireReach(ctx({ reach: scoped("clovis") }), "fresno");
    expect(refused?.status).toBe(403);
    expect(await refused?.json()).toEqual({
      error: "you are not rostered at fresno",
      code: "OutOfReach",
      locationId: "fresno"
    });
  });
});

describe("predicate", () => {
  it("is TRUE for everywhere and binds nothing", () => {
    const params: unknown[] = ["x"];
    expect(
      predicate(
        { everywhere: true, via: "app-admin", locations: ["clovis"] },
        "t.location_id",
        params
      )
    ).toBe("TRUE");
    expect(params).toEqual(["x"]);
  });

  it("binds the locations as one text parameter numbered after the existing ones", () => {
    const params: unknown[] = ["x", "y"];
    expect(predicate(scoped("clovis", "fresno"), "t.location_id", params)).toBe(
      "t.location_id::text = ANY(string_to_array($3::text, ','))"
    );
    expect(params).toEqual(["x", "y", "clovis,fresno"]);
  });

  it("casts the column to text, so a uuid column compares to the text[] string_to_array yields", () => {
    // The platform's location ids are uuids; `uuid = text[]` has no operator
    // in Postgres, and a reader that never runs is a 500 that points nowhere.
    const params: unknown[] = [];
    const sql = predicate(scoped("2e7a5d1e-6d1e-4a8b-9c1f-0d2b3a4c5e6f"), "l.id", params);
    expect(sql.startsWith("l.id::text = ANY(")).toBe(true);
  });

  it("an empty list binds an empty string, which string_to_array makes an empty array", () => {
    const params: unknown[] = [];
    expect(predicate(scoped(), "location_id", params)).toBe(
      "location_id::text = ANY(string_to_array($1::text, ','))"
    );
    expect(params).toEqual([""]);
  });
});

describe("isAdmin / adminOnly", () => {
  it("reads appRole and nothing else", () => {
    expect(isAdmin(ctx({ appRole: "admin" }))).toBe(true);
    expect(isAdmin(ctx({ appRole: "member" }))).toBe(false);
    expect(isAdmin(ctx({}))).toBe(false);
  });

  it("refuses with a stable code", async () => {
    const r = adminOnly("editing the roster");
    expect(r.status).toBe(403);
    expect(await r.json()).toEqual({
      error: "editing the roster needs app-admin standing",
      code: "AdminOnly"
    });
  });
});
