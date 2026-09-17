import { describe, expect, it } from "vitest";
import type { AppHealth, CustomApp } from "@/types/apps";
import {
  type AppsTableState,
  buildAppsTableModel,
  DEFAULT_TABLE_STATE,
  statusOf
} from "./useAppsTable";

/** Minimal CustomApp with sensible defaults; override per test. */
function app(over: Partial<CustomApp>): CustomApp {
  return {
    id: over.id ?? over.slug ?? "id",
    slug: "app",
    name: "App",
    org_id: "org-id",
    org_slug: "acme",
    project_id: "proj-0000",
    branch: "main",
    source_repo: "",
    status: "active",
    url: "/customer-apps/acme/app/",
    url_subdomain: null,
    source_type: "s3",
    source_config: {},
    bootstrap_pr_url: null,
    last_synced_at: null,
    last_deploy_at: null,
    published_at: null,
    repo_path: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...over
  };
}

const state = (over: Partial<AppsTableState> = {}): AppsTableState => ({
  ...DEFAULT_TABLE_STATE,
  ...over
});

describe("buildAppsTableModel", () => {
  it("filters by search across name / slug / org / project", () => {
    const apps = [
      app({ id: "a", name: "Sales", slug: "sales", org_slug: "acme" }),
      app({ id: "b", name: "Ops", slug: "ops", org_slug: "globex" })
    ];
    expect(buildAppsTableModel(apps, state({ q: "globex", group: "none" })).flatIds).toEqual(["b"]);
    expect(buildAppsTableModel(apps, state({ q: "sales", group: "none" })).flatIds).toEqual(["a"]);
  });

  it("filters by status (live = has published_at)", () => {
    const apps = [
      app({ id: "live", published_at: "2026-02-01T00:00:00Z" }),
      app({ id: "draft", published_at: null })
    ];
    expect(buildAppsTableModel(apps, state({ status: "live", group: "none" })).flatIds).toEqual([
      "live"
    ]);
    expect(buildAppsTableModel(apps, state({ status: "draft", group: "none" })).flatIds).toEqual([
      "draft"
    ]);
  });

  it("groups by org and orders groups by their top sorted row", () => {
    const apps = [
      app({ id: "old", org_slug: "acme", updated_at: "2026-01-01T00:00:00Z" }),
      app({ id: "new", org_slug: "globex", updated_at: "2026-06-01T00:00:00Z" })
    ];
    // Explicit sort: this pins that group order follows the sort, not which
    // sort happens to be the default.
    const model = buildAppsTableModel(
      apps,
      state({ group: "org", sortKey: "updated", sortDir: "desc" })
    );
    expect(model.groups.map((g) => g.key)).toEqual(["globex", "acme"]);
    expect(model.flatIds).toEqual(["new", "old"]);
  });

  it("keeps flatIds aligned with visual group order for shift-select", () => {
    const apps = [
      app({ id: "a1", org_slug: "acme", name: "A1" }),
      app({ id: "g1", org_slug: "globex", name: "G1" }),
      app({ id: "a2", org_slug: "acme", name: "A2" })
    ];
    const model = buildAppsTableModel(
      apps,
      state({ group: "org", sortKey: "name", sortDir: "asc" })
    );
    // Grouped by org: acme rows contiguous, then globex — flatIds mirrors that.
    expect(model.flatIds).toEqual(["a1", "a2", "g1"]);
  });

  it("sorts by name ascending", () => {
    const apps = [app({ id: "b", name: "Beta" }), app({ id: "a", name: "Alpha" })];
    const model = buildAppsTableModel(
      apps,
      state({ group: "none", sortKey: "name", sortDir: "asc" })
    );
    expect(model.flatIds).toEqual(["a", "b"]);
  });

  it("reports filtered and total counts", () => {
    const apps = [app({ id: "a", name: "keep" }), app({ id: "b", name: "drop" })];
    const model = buildAppsTableModel(apps, state({ q: "keep" }));
    expect(model.filteredCount).toBe(1);
    expect(model.totalCount).toBe(2);
  });
});

const LIVE = "2026-02-01T00:00:00Z";
const health = (entries: Record<string, AppHealth>) =>
  new Map(Object.entries(entries).map(([id, h]) => [id, { health: h }]));

describe("status", () => {
  it("is draft for an unpublished app, whatever the health data says", () => {
    const a = app({ id: "d", published_at: null });
    expect(statusOf(a, health({ d: "down" }))).toBe("draft");
  });

  /** The fleet view exists to stop calling an unasked-about app unmeasured. A
   *  published app missing from the health data is unknown — still loading, or
   *  past the page cap — and must not borrow a verdict. */
  it("is unknown, not not_measured, for a published app with no health entry", () => {
    const a = app({ id: "p", published_at: LIVE });
    expect(statusOf(a, undefined)).toBeNull();
    expect(statusOf(a, health({}))).toBeNull();
    expect(statusOf(a, health({ p: "quiet" }))).toBe("quiet");
  });

  it("sorts worst first by default: verdicts, then unknown, then drafts", () => {
    const apps = [
      app({ id: "draft", published_at: null }),
      app({ id: "ok", published_at: LIVE }),
      app({ id: "unknown", published_at: LIVE }),
      app({ id: "quiet", published_at: LIVE }),
      app({ id: "unmeasured", published_at: LIVE }),
      app({ id: "degraded", published_at: LIVE }),
      app({ id: "down", published_at: LIVE })
    ];
    const h = health({
      ok: "operational",
      quiet: "quiet",
      unmeasured: "not_measured",
      degraded: "degraded",
      down: "down"
    });
    expect(buildAppsTableModel(apps, state(), h).flatIds).toEqual([
      "down",
      "degraded",
      "unmeasured",
      "quiet",
      "ok",
      "unknown",
      "draft"
    ]);
  });

  it("filters to the three verdicts that need someone, and nothing unknown", () => {
    const apps = [
      app({ id: "down", published_at: LIVE }),
      app({ id: "unmeasured", published_at: LIVE }),
      app({ id: "ok", published_at: LIVE }),
      app({ id: "unknown", published_at: LIVE }),
      app({ id: "draft", published_at: null })
    ];
    const h = health({ down: "down", unmeasured: "not_measured", ok: "operational" });
    expect(buildAppsTableModel(apps, state({ status: "attention" }), h).flatIds).toEqual([
      "down",
      "unmeasured"
    ]);
  });

  it("keeps an old ?status=live meaning 'not a draft', unknown included", () => {
    const apps = [
      app({ id: "unknown", published_at: LIVE }),
      app({ id: "draft", published_at: null })
    ];
    expect(buildAppsTableModel(apps, state({ status: "live" })).flatIds).toEqual(["unknown"]);
  });

  /** A chip's number says what clicking it would show. If the counts followed
   *  the status filter, choosing "Down" would zero every other chip. */
  it("counts statuses over the other filters, ignoring the status filter", () => {
    const apps = [
      app({ id: "d1", org_slug: "acme", published_at: LIVE }),
      app({ id: "d2", org_slug: "globex", published_at: LIVE }),
      app({ id: "q", org_slug: "acme", published_at: LIVE }),
      app({ id: "draft", org_slug: "acme", published_at: null })
    ];
    const h = health({ d1: "down", d2: "down", q: "quiet" });
    const model = buildAppsTableModel(apps, state({ status: "down", org: "acme" }), h);
    expect(model.flatIds).toEqual(["d1"]);
    expect(model.statusCounts).toMatchObject({ down: 1, quiet: 1, draft: 1, attention: 1 });
  });

  it("filters by org, and lists every org regardless of the filters", () => {
    const apps = [app({ id: "a", org_slug: "acme" }), app({ id: "g", org_slug: "globex" })];
    const model = buildAppsTableModel(apps, state({ org: "globex" }));
    expect(model.flatIds).toEqual(["g"]);
    expect(model.orgs).toEqual(["acme", "globex"]);
  });

  /** The landing is the redesign's decision: one ungrouped list, worst first.
   *  Grouping by org cut worst-first into one short run per tenant. */
  it("lands on an ungrouped list, worst first", () => {
    expect(DEFAULT_TABLE_STATE).toMatchObject({
      view: "list",
      group: "none",
      sortKey: "status",
      sortDir: "asc"
    });
  });
});
