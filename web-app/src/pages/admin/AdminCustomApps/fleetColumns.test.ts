import { describe, expect, it } from "vitest";
import type { AppStorageUsageRow, CustomApp } from "@/types/apps";
import { type FleetColumnId, fleetColumns } from "./fleetColumns";

/**
 * The specification for "two of five columns were structurally empty".
 *
 * These tests are written against the *reason* a column is absent, not just its
 * absence — because the failure mode being prevented is a column that renders `—` on
 * every row forever while looking like it might one day say something.
 */

const app = (over: Partial<CustomApp> = {}): CustomApp =>
  ({
    id: over.id ?? "a",
    slug: "oxy-starter",
    name: "Oxy Starter",
    org_slug: "acme",
    published_at: "2026-09-19T17:30:27Z",
    last_active_at: null,
    ...over
  }) as CustomApp;

const storageRow = (appId: string): AppStorageUsageRow =>
  ({ appId, bytes: 6_500_000_000, objectCount: 3132 }) as AppStorageUsageRow;

const ids = (cols: { id: FleetColumnId }[]) => cols.map((c) => c.id);

const columns = (over: Partial<Parameters<typeof fleetColumns>[0]> = {}) =>
  fleetColumns({
    apps: [app()],
    observabilityConfigured: false,
    storage: undefined,
    ...over
  });

describe("fleetColumns", () => {
  it("never derives away identity or the verdict, even on an empty fleet", () => {
    // A list that cannot say which app, or how it is, has stopped being a fleet list.
    expect(ids(columns({ apps: [] }).shown)).toEqual(["status", "app"]);
  });

  it("drops Requests when nothing on the deployment is measured", () => {
    const { shown, hidden } = columns({ observabilityConfigured: false });
    expect(ids(shown)).not.toContain("requests");
    expect(hidden).toContainEqual({
      label: "Requests",
      why: "nothing on this deployment is measured"
    });
  });

  it("keeps Requests when capture is on, even though every app sits at zero", () => {
    // The exception the module exists for: with capture ON, all-zero requests is real
    // information ("nobody used any of these"). Reading the values alone cannot tell
    // that apart from capture being off, so the flag decides and the values do not.
    const { shown, hidden } = columns({ observabilityConfigured: true });
    expect(ids(shown)).toContain("requests");
    expect(hidden.map((h) => h.label)).not.toContain("Requests");
  });

  it("drops Last active until some app has actually recorded a visit", () => {
    expect(ids(columns({ apps: [app({ last_active_at: null })] }).shown)).not.toContain(
      "lastActive"
    );
    expect(
      ids(columns({ apps: [app({ last_active_at: "2026-09-21T09:00:00Z" })] }).shown)
    ).toContain("lastActive");
  });

  it("shows a column when only ONE app of several can answer it", () => {
    // "At least one", not "all" — a column that waits for every row to have a value
    // would hide the very rows worth looking at.
    const { shown } = columns({
      apps: [
        app({ id: "a", last_active_at: null }),
        app({ id: "b", last_active_at: "2026-09-21T09:00:00Z" }),
        app({ id: "c", last_active_at: null })
      ]
    });
    expect(ids(shown)).toContain("lastActive");
  });

  it("drops Published when nothing has been published", () => {
    const { shown, hidden } = columns({ apps: [app({ published_at: null })] });
    expect(ids(shown)).not.toContain("published");
    expect(hidden.map((h) => h.why)).toContain("nothing here has been published yet");
  });

  it("drops Storage while the rollup has not arrived, and shows it once it has", () => {
    expect(ids(columns({ storage: undefined }).shown)).not.toContain("storage");
    expect(ids(columns({ storage: new Map([["a", storageRow("a")]]) }).shown)).toContain("storage");
  });

  it("does not show Storage when the rollup arrived but holds no row for these apps", () => {
    // A loaded-but-irrelevant rollup is not an answer for this fleet.
    const { shown } = columns({
      apps: [app({ id: "a" })],
      storage: new Map([["someone-else", storageRow("someone-else")]])
    });
    expect(ids(shown)).not.toContain("storage");
  });

  it("names every dropped column exactly once, so nothing vanishes silently", () => {
    const { shown, hidden } = columns();
    const labels = hidden.map((h) => h.label);
    expect(new Set(labels).size).toBe(labels.length);
    // Every column is accounted for: either on screen or explained.
    expect(shown.length + hidden.length).toBe(6);
    for (const h of hidden) expect(h.why).not.toBe("");
  });
});
