import { describe, expect, it } from "vitest";
import type { AppHealth, CustomApp } from "@/types/apps";
import type { HealthIndex } from "./appStatus";
import { discriminatingFilters } from "./fleetFilters";

/**
 * The specification for "All 3 · Needs attention 3 · Not measured 3" — three chips
 * that all selected the same three apps.
 *
 * The rule under test is `0 < n < total`. Everything else here is a consequence of it.
 */

const app = (id: string, over: Partial<CustomApp> = {}): CustomApp =>
  ({
    id,
    slug: `app-${id}`,
    name: `App ${id}`,
    org_slug: "acme",
    published_at: "2026-09-19T17:30:27Z",
    ...over
  }) as CustomApp;

const health = (...pairs: Array<[string, AppHealth]>): HealthIndex =>
  new Map(pairs.map(([id, h]) => [id, { health: h }]));

describe("discriminatingFilters", () => {
  it("offers nothing when every app shares one status — the case that shipped", () => {
    // Today's seeded fleet exactly: three apps, all not_measured. A chip here cannot
    // change what is on screen, so the row does not render at all.
    const apps = [app("a"), app("b"), app("c")];
    const h = health(["a", "not_measured"], ["b", "not_measured"], ["c", "not_measured"]);
    expect(discriminatingFilters(apps, h)).toEqual([]);
  });

  it("offers a chip that selects a proper subset", () => {
    const apps = [app("a"), app("b"), app("c")];
    const h = health(["a", "down"], ["b", "operational"], ["c", "operational"]);
    const filters = discriminatingFilters(apps, h);
    expect(filters.map((f) => [f.status, f.count])).toEqual([
      ["down", 1],
      ["operational", 2]
    ]);
  });

  it("never offers an All chip, because clearing a filter is the absence of one", () => {
    const apps = [app("a"), app("b")];
    const h = health(["a", "down"], ["b", "operational"]);
    const filters = discriminatingFilters(apps, h);
    expect(filters.map((f) => f.label)).not.toContain("All");
    // and no chip may match the whole fleet under another name
    for (const f of filters) expect(f.count).toBeLessThan(apps.length);
  });

  it("sorts worst first, so the chip worth clicking is the leftmost", () => {
    const apps = [app("a"), app("b"), app("c"), app("d")];
    const h = health(["a", "operational"], ["b", "degraded"], ["c", "down"], ["d", "operational"]);
    expect(discriminatingFilters(apps, h).map((f) => f.status)).toEqual([
      "down",
      "degraded",
      "operational"
    ]);
  });

  it("does not offer a chip for apps whose status is not known yet", () => {
    // `statusOf` returns null while health is in flight. A chip for that would invite
    // filtering on the absence of an answer — and would flicker into existence and
    // back out as the fetch lands.
    const apps = [app("a"), app("b"), app("c")];
    const h = health(["a", "down"]); // b and c unknown
    const filters = discriminatingFilters(apps, h);
    expect(filters.map((f) => f.status)).toEqual(["down"]);
    expect(filters.reduce((n, f) => n + f.count, 0)).toBe(1);
  });

  it("counts an unpublished app as a draft rather than by its health", () => {
    const apps = [app("a", { published_at: null }), app("b"), app("c")];
    const h = health(["a", "operational"], ["b", "operational"], ["c", "operational"]);
    expect(discriminatingFilters(apps, h).map((f) => [f.status, f.count])).toEqual([
      ["operational", 2],
      ["draft", 1]
    ]);
  });

  it("offers nothing for a fleet of one, whatever its status", () => {
    // One app cannot be narrowed to a proper subset of itself.
    expect(discriminatingFilters([app("a")], health(["a", "down"]))).toEqual([]);
  });
});
