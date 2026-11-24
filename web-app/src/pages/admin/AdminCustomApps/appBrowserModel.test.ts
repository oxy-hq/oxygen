import { describe, expect, it } from "vitest";
import type { AppHealth, CustomApp } from "@/types/apps";
import {
  appsInScope,
  type OrgRow,
  orgRows,
  rollup,
  scopeFrom,
  typeAheadTarget
} from "./appBrowserModel";
import type { HealthIndex } from "./appStatus";

/**
 * The specification for the column browser's two derived tiers.
 *
 * Written against the *reasons* in `appBrowserModel`'s header rather than against the
 * current output: that unknown health is never an all-clear, that a column's order does
 * not depend on health (because health repolls under the cursor), and that a stale
 * `?org=` cannot produce a tier with nothing in it.
 */

const app = (over: Partial<CustomApp> = {}): CustomApp =>
  ({
    id: over.id ?? over.slug ?? "a",
    slug: "oxy-starter",
    name: "Oxy Starter",
    org_slug: "acme",
    published_at: "2026-09-19T17:30:27Z",
    ...over
  }) as CustomApp;

const health = (entries: Record<string, AppHealth>): HealthIndex =>
  new Map(Object.entries(entries).map(([id, h]) => [id, { health: h }]));

describe("rollup", () => {
  it("counts only the apps a person has to look at", () => {
    const apps = [
      app({ id: "down", slug: "a" }),
      app({ id: "ok", slug: "b" }),
      app({ id: "quiet", slug: "c" }),
      app({ id: "draft", slug: "d", published_at: null })
    ];
    const index = health({ down: "down", ok: "operational", quiet: "quiet" });
    // `down` needs a person; operational, quiet and draft do not.
    expect(rollup(apps, index)).toEqual({ appCount: 4, attention: 1, worst: "down" });
  });

  it("reports the worst verdict, not the first or the last", () => {
    const apps = [app({ id: "a", slug: "a" }), app({ id: "b", slug: "b" })];
    expect(rollup(apps, health({ a: "operational", b: "degraded" })).worst).toBe("degraded");
    expect(rollup(apps, health({ a: "degraded", b: "operational" })).worst).toBe("degraded");
  });

  it("does not let unknown health beat a real verdict", () => {
    // One app is down, the other has not been measured. "Down" is the fact.
    const apps = [app({ id: "a", slug: "a" }), app({ id: "b", slug: "b" })];
    expect(rollup(apps, health({ a: "down" })).worst).toBe("down");
  });

  it("does not let a draft outrank a verdict, and ranks unknown above one", () => {
    const published = app({ id: "a", slug: "a" });
    const draft = app({ id: "b", slug: "b", published_at: null });
    // A draft serves nobody, so it is never the worst thing about a set that has a
    // measured app in it — not even an unmeasured one.
    expect(rollup([published, draft], undefined).worst).toBe(null);
    expect(rollup([published, draft], health({ a: "quiet" })).worst).toBe("quiet");
  });

  it("says nothing is known rather than claiming an all-clear", () => {
    // The whole reason `statusOf` returns null: health in flight and health off look the
    // same from here, and neither is a verdict that the org is fine.
    expect(rollup([app()], undefined).worst).toBe(null);
    expect(rollup([], undefined)).toEqual({ appCount: 0, attention: 0, worst: null });
  });
});

describe("orgRows", () => {
  const apps = [
    app({ id: "1", org_slug: "mars", slug: "command-center" }),
    app({ id: "2", org_slug: "acme", slug: "store-ops" }),
    app({ id: "3", org_slug: "acme", slug: "oxy-starter" })
  ];

  it("gives one row per org that has an app, with its own count", () => {
    expect(orgRows(apps, undefined).map((o) => [o.orgSlug, o.appCount])).toEqual([
      ["acme", 2],
      ["mars", 1]
    ]);
  });

  it("orders by slug, and that order does not move when health changes", () => {
    // The point of the whole ordering decision: a column must not re-sort under an
    // operator's cursor every time the minute-ly health poll lands.
    const before = orgRows(apps, undefined).map((o) => o.orgSlug);
    const after = orgRows(apps, health({ "1": "down", "2": "operational" })).map((o) => o.orgSlug);
    expect(after).toEqual(before);
    expect(after).toEqual(["acme", "mars"]);
  });

  it("carries the org's own attention count, not the deployment's", () => {
    const rows = orgRows(apps, health({ "1": "down", "2": "degraded", "3": "operational" }));
    expect(rows.map((o) => [o.orgSlug, o.attention, o.worst])).toEqual([
      ["acme", 1, "degraded"],
      ["mars", 1, "down"]
    ]);
  });
});

describe("appsInScope", () => {
  const apps = [
    app({ id: "1", org_slug: "mars", slug: "command-center", name: "Command Center" }),
    app({ id: "2", org_slug: "acme", slug: "store-ops", name: "Store Ops" }),
    app({ id: "3", org_slug: "acme", slug: "oxy-starter", name: "Oxy Starter" })
  ];

  it("shows every app under All apps, by name", () => {
    expect(appsInScope(apps, { kind: "all" }).map((a) => a.name)).toEqual([
      "Command Center",
      "Oxy Starter",
      "Store Ops"
    ]);
  });

  it("narrows to one org", () => {
    expect(appsInScope(apps, { kind: "org", orgSlug: "acme" }).map((a) => a.slug)).toEqual([
      "oxy-starter",
      "store-ops"
    ]);
  });

  it("breaks a shared name on org then slug, so the order is stable", () => {
    // `oxy-starter` really does exist under two orgs — the ambiguity `AppIdentity`
    // exists for. Two rows reading "Oxy Starter" must at least not swap places.
    const twins = [
      app({ id: "1", org_slug: "local", slug: "oxy-starter", name: "Oxy Starter" }),
      app({ id: "2", org_slug: "acme", slug: "oxy-starter", name: "Oxy Starter" })
    ];
    expect(appsInScope(twins, { kind: "all" }).map((a) => a.org_slug)).toEqual(["acme", "local"]);
  });
});

describe("scopeFrom", () => {
  const orgs: OrgRow[] = [
    { orgSlug: "acme", appCount: 2, attention: 0, worst: null },
    { orgSlug: "mars", appCount: 1, attention: 0, worst: null }
  ];

  it("follows the selected app, ignoring a stale ?org=", () => {
    const selected = app({ org_slug: "mars" });
    expect(scopeFrom(selected, "acme", orgs)).toEqual({ kind: "org", orgSlug: "mars" });
  });

  it("reads ?org= only when no app is selected", () => {
    expect(scopeFrom(null, "acme", orgs)).toEqual({ kind: "org", orgSlug: "acme" });
    expect(scopeFrom(null, null, orgs)).toEqual({ kind: "all" });
  });

  it("falls back to All apps for an org nothing is filed under", () => {
    // A stale link should land somewhere real, not open a tier that cannot have rows.
    expect(scopeFrom(null, "deleted-org", orgs)).toEqual({ kind: "all" });
  });
});

describe("typeAheadTarget", () => {
  const labels = ["acme", "local", "mars", "mars-eu"];

  it("cycles through the matches when one key is pressed repeatedly", () => {
    expect(typeAheadTarget(labels, "m", 0)).toBe(2);
    expect(typeAheadTarget(labels, "m", 2)).toBe(3);
    // …and wraps rather than stopping at the end.
    expect(typeAheadTarget(labels, "m", 3)).toBe(2);
  });

  it("refines onto the row it is already on once the buffer grows", () => {
    // Pressing m-a-r-s from row 2 must stay on `mars`, not skip to `mars-eu`.
    expect(typeAheadTarget(labels, "mars", 2)).toBe(2);
  });

  it("reaches a row before the cursor by wrapping", () => {
    expect(typeAheadTarget(labels, "a", 3)).toBe(0);
  });

  it("matches on a prefix, case-insensitively, and nothing else", () => {
    expect(typeAheadTarget(["Store Ops"], "st", 0)).toBe(0);
    // Not a substring search: "ops" does not select "Store Ops". Finder's behaviour, and
    // the one that makes repeated keys predictable.
    expect(typeAheadTarget(["Store Ops"], "ops", 0)).toBe(null);
  });

  it("answers null rather than guessing", () => {
    expect(typeAheadTarget(labels, "zz", 0)).toBe(null);
    expect(typeAheadTarget([], "a", 0)).toBe(null);
    expect(typeAheadTarget(labels, "", 0)).toBe(null);
  });
});
