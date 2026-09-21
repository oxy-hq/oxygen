import { describe, expect, it } from "vitest";
import type { AppHealth, CustomApp } from "@/types/apps";
import {
  ATTENTION,
  byAttention,
  type HealthIndex,
  matchesQuery,
  needsAttention,
  statusOf,
  statusRank
} from "./appStatus";

/**
 * The judgement salvaged from the deleted registry table. Two things matter more than
 * the ordering itself:
 *
 * - **unknown is not a verdict.** `statusOf` returns `null` when health has not
 *   arrived, and nothing may turn that into `not_measured` — that would claim capture
 *   is off for an app nobody asked about.
 * - **`byAttention` orders the fleet**, so its first row is what an operator reads as
 *   "the thing to look at". A tie that reorders between reloads would reshuffle the
 *   list under them. It was briefly load-bearing *navigation* too — the bare route
 *   redirected to `[0]` — and the ordering outlived that redirect unchanged.
 */

const app = (over: Partial<CustomApp> = {}): CustomApp =>
  ({
    id: over.id ?? `id-${Math.random()}`,
    slug: "oxy-starter",
    name: "Oxy Starter",
    org_slug: "acme",
    published_at: "2026-09-19T17:30:27Z",
    ...over
  }) as CustomApp;

const health = (...pairs: Array<[string, AppHealth]>): HealthIndex =>
  new Map(pairs.map(([id, h]) => [id, { health: h }]));

describe("statusOf", () => {
  it("calls an unpublished app a draft before it looks at health at all", () => {
    const a = app({ id: "a", published_at: null });
    expect(statusOf(a, health(["a", "operational"]))).toBe("draft");
  });

  it("reports the health verdict for a published app", () => {
    const a = app({ id: "a" });
    expect(statusOf(a, health(["a", "degraded"]))).toBe("degraded");
  });

  it("returns null — not a verdict — when health has not arrived", () => {
    expect(statusOf(app({ id: "a" }), undefined)).toBeNull();
    expect(statusOf(app({ id: "a" }), health(["other", "operational"]))).toBeNull();
  });
});

describe("needsAttention", () => {
  it("wants a person for exactly the three verdicts the backend names", () => {
    for (const s of ["down", "degraded", "not_measured"] as const) {
      expect(ATTENTION.has(s)).toBe(true);
      expect(needsAttention(s)).toBe(true);
    }
  });

  it("does not want a person for a healthy, quiet or draft app", () => {
    for (const s of ["operational", "quiet", "draft"] as const)
      expect(needsAttention(s)).toBe(false);
  });

  it("does not want a person for an app nobody measured", () => {
    // `null` means "we do not know", which is not the same as "not measured" — that
    // distinction is the whole reason `statusOf` can return null.
    //
    // Honest note: this assertion does NOT pin the `s !== null` guard in the
    // implementation. `Set.prototype.has(null)` is already false, so deleting the guard
    // leaves runtime behaviour identical and this test still green — verified by
    // mutating it. The guard is there for the type checker (`ATTENTION.has` takes
    // `AppStatus`, not `AppStatus | null`), and the type checker is what pins it.
    expect(needsAttention(null)).toBe(false);
  });
});

describe("statusRank", () => {
  it("ranks worst first, with unknown between the verdicts and drafts", () => {
    const order = (
      ["down", "degraded", "not_measured", "quiet", "operational", null, "draft"] as const
    ).map(statusRank);
    expect(order).toEqual([...order].sort((a, b) => a - b));
    expect(statusRank(null)).toBeGreaterThan(statusRank("operational"));
    expect(statusRank(null)).toBeLessThan(statusRank("draft"));
  });
});

describe("byAttention", () => {
  it("puts the app that most needs someone first — this is what /admin/apps opens on", () => {
    const down = app({ id: "d", name: "Zeta" });
    const fine = app({ id: "f", name: "Alpha" });
    const sorted = byAttention([fine, down], health(["d", "down"], ["f", "operational"]));
    expect(sorted[0].id).toBe("d");
  });

  it("breaks ties on name then org, so a reload lands in the same place", () => {
    const h = health(["1", "down"], ["2", "down"], ["3", "down"]);
    const apps = [
      app({ id: "1", name: "Beta", org_slug: "acme" }),
      app({ id: "3", name: "Alpha", org_slug: "zzz" }),
      app({ id: "2", name: "Alpha", org_slug: "acme" })
    ];
    const once = byAttention(apps, h).map((a) => a.id);
    const twice = byAttention([...apps].reverse(), h).map((a) => a.id);
    expect(once).toEqual(["2", "3", "1"]);
    expect(twice).toEqual(once);
  });

  it("does not mutate the fleet it was handed", () => {
    const apps = [app({ id: "a", name: "Z" }), app({ id: "b", name: "A" })];
    byAttention(apps, undefined);
    expect(apps.map((a) => a.id)).toEqual(["a", "b"]);
  });

  it("still orders a fleet whose health never arrived", () => {
    const apps = [app({ id: "a", name: "Z" }), app({ id: "b", name: "A" })];
    expect(byAttention(apps, undefined).map((a) => a.name)).toEqual(["A", "Z"]);
  });
});

describe("matchesQuery", () => {
  const a = app({ name: "Oxy Starter", slug: "oxy-starter", org_slug: "acme" });

  it("matches on name, slug and org, because name alone does not identify an app", () => {
    for (const q of ["oxy", "starter", "acme", "OXY", "  acme  "])
      expect(matchesQuery(a, q)).toBe(true);
  });

  it("matches the compound org/slug an operator would paste from a URL", () => {
    expect(matchesQuery(a, "acme/oxy-starter")).toBe(true);
  });

  it("narrows between two apps that share a name", () => {
    const local = app({ name: "Oxy Starter", slug: "oxy-starter", org_slug: "local" });
    expect(matchesQuery(a, "acme")).toBe(true);
    expect(matchesQuery(local, "acme")).toBe(false);
  });

  it("matches everything on an empty query", () => {
    expect(matchesQuery(a, "")).toBe(true);
    expect(matchesQuery(a, "   ")).toBe(true);
  });
});
