import { describe, expect, it } from "vitest";
import type { Fleet, FleetRow } from "@/services/api/adminObservability";
import {
  countToReach,
  isCostJump,
  isIdle,
  JUMP_FLOOR_USD,
  matchesQuery,
  priorNote,
  summarizeFleet
} from "./fleetSummary";

function row(name: string, cost: number, prior: number | null, extra: Partial<FleetRow> = {}) {
  return {
    org_id: name,
    org_name: name,
    org_slug: name.toLowerCase(),
    cost_usd: cost,
    run_count: cost > 0 ? 1 : 0,
    unpriced_runs: 0,
    input_tokens: 0,
    output_tokens: 0,
    cost_per_run_usd: null,
    monthly_run_rate_usd: null,
    unattended_run_share: null,
    cache_hit_share: null,
    requests: 0,
    sessions: 0,
    people: 0,
    apps: 0,
    p95_ms: null,
    failure_rate: null,
    function_compute_gbs: null,
    egress_gb: null,
    storage_gib_month: null,
    storage_usd: null,
    egress_usd: 0,
    compute_usd: null,
    cost_to_serve_usd: cost,
    prior_cost_to_serve_usd: prior,
    ...extra
  } satisfies FleetRow;
}

function fleet(rows: FleetRow[], prior_read = true): Fleet {
  return {
    window_days: 30,
    apps_measured: true,
    storage_read: true,
    compute_priced: false,
    prior_read,
    prior_missing: prior_read ? null : "read_failed",
    provisioned_memory_mb: 128,
    rows
  };
}

describe("summarizeFleet", () => {
  it("ranks active orgs by cost to serve and sets idle ones aside", () => {
    const s = summarizeFleet(
      fleet([row("B", 10, 10), row("A", 30, 5), row("Idle", 0, 0), row("C", 10, 0)])
    );
    expect(s.ranked.map((o) => o.row.org_name)).toEqual(["A", "B", "C"]);
    expect(s.ranked.map((o) => o.rank)).toEqual([1, 2, 3]);
    expect(s.idle.map((r) => r.org_name)).toEqual(["Idle"]);
    expect(s.total).toBe(50);
  });

  it("measures movement against the prior window's ranks", () => {
    // Prior order: B (10), A (5). Now: A, B, C.
    const s = summarizeFleet(fleet([row("A", 30, 5), row("B", 10, 10), row("C", 10, 0)]));
    const byName = Object.fromEntries(s.ranked.map((o) => [o.row.org_name, o]));
    expect(byName.A.movement).toBe(1);
    expect(byName.B.movement).toBe(-1);
    // Cost nothing before: new, not "up from last place".
    expect(byName.C.movement).toBe("new");
  });

  it("draws no comparison when the prior window could not be read", () => {
    const s = summarizeFleet(fleet([row("A", 30, null), row("B", 10, null)], false));
    expect(s.priorTotal).toBeNull();
    expect(s.versus).toBeNull();
    expect(s.ranked.every((o) => o.movement === null && o.versus === null)).toBe(true);
  });

  it("reports change against a non-zero prior only", () => {
    const s = summarizeFleet(fleet([row("A", 30, 10), row("B", 10, 0)]));
    expect(s.versus).toEqual({ kind: "pct", change: 3 });
    expect(s.ranked[0].versus).toEqual({ kind: "pct", change: 2 });
    // From zero there is no ratio: "new" carries it instead.
    expect(s.ranked[1].versus).toEqual({ kind: "new", prior: 0 });
  });

  /**
   * Dev on 2026-11-18, the fleet that read "up 231432043622%": three orgs
   * that started spending, and two whose only cost, then and now, was a few
   * billionths of a dollar of storage.
   */
  const devFleet = () =>
    fleet([
      row("Oxygen", 46.0961664, 0),
      row("July", 0.50585565, 0),
      row("Testzxfcv", 0.04658775, 0),
      row("Pokehouse", 1.672924414e-8, 1.672925705e-8, { requests: 2296 }),
      row("Delightree", 3.427245919e-9, 3.427247241e-9, { requests: 2929 })
    ]);

  it("compares the total with nothing when the prior total is negligible", () => {
    const s = summarizeFleet(devFleet());
    expect(s.priorTotal).toBeGreaterThan(0);
    expect(s.versus?.kind).toBe("new");
  });

  it("ranks no org against a prior it barely had", () => {
    const s = summarizeFleet(devFleet());
    const byName = Object.fromEntries(s.ranked.map((o) => [o.row.org_name, o]));
    // Was "↓3" and "±0%": a fall from first place earned on $0.00000002.
    for (const name of ["Pokehouse", "Delightree"]) {
      expect(byName[name].movement).toBe("new");
      expect(byName[name].versus?.kind).toBe("new");
    }
  });

  it("still ranks orgs whose prior was a real amount", () => {
    // Prior order: B ($0.02), A ($0.01); C's $0.009 is not a place.
    const s = summarizeFleet(fleet([row("A", 3, 0.01), row("B", 2, 0.02), row("C", 1, 0.009)]));
    const byName = Object.fromEntries(s.ranked.map((o) => [o.row.org_name, o]));
    expect(byName.A.movement).toBe(1);
    expect(byName.B.movement).toBe(-1);
    expect(byName.C.movement).toBe("new");
  });

  it("states how concentrated the total is", () => {
    const s = summarizeFleet(
      fleet([row("A", 50, 0), row("B", 30, 0), row("C", 10, 0), row("D", 10, 0)])
    );
    expect(s.toHalf).toBe(1);
    expect(s.toEighty).toBe(2);
  });

  it("counts an org kept active by app traffic alone", () => {
    expect(isIdle(row("Apps", 0, 0, { requests: 12 }))).toBe(false);
    expect(isIdle(row("Quiet", 0, 0, { requests: null }))).toBe(true);
  });
});

describe("countToReach", () => {
  it("is null for a fleet that cost nothing", () => {
    expect(countToReach([0, 0], 0, 0.5)).toBeNull();
  });
  it("counts a sum landing exactly on the threshold", () => {
    expect(countToReach([0.1, 0.2, 0.2], 0.5, 0.6)).toBe(2);
  });
});

describe("isCostJump", () => {
  it("flags a doubling only above the floor", () => {
    const s = summarizeFleet(
      fleet([row("Big", JUMP_FLOOR_USD * 4, JUMP_FLOOR_USD), row("Tiny", 0.07, 0.03)])
    );
    const byName = Object.fromEntries(s.ranked.map((o) => [o.row.org_name, o]));
    expect(isCostJump(byName.Big)).toBe(true);
    expect(isCostJump(byName.Tiny)).toBe(false);
  });

  it("flags growth past tenfold, which reads as a baseline rather than a percentage", () => {
    const s = summarizeFleet(fleet([row("Tenfold", 47, 0.5)]));
    expect(s.ranked[0].versus).toEqual({ kind: "from", prior: 0.5 });
    expect(isCostJump(s.ranked[0])).toBe(true);
  });

  it("does not flag an org whose prior was negligible: it is new, not a jump", () => {
    const s = summarizeFleet(fleet([row("Fresh", 47, 2e-8)]));
    expect(isCostJump(s.ranked[0])).toBe(false);
  });
});

describe("matchesQuery", () => {
  it("matches name or slug, case-insensitively", () => {
    const r = row("Poke House", 1, 1, { org_slug: "poke-house" });
    expect(matchesQuery(r, "POKE")).toBe(true);
    expect(matchesQuery(r, "poke-h")).toBe(true);
    expect(matchesQuery(r, "bmg")).toBe(false);
    expect(matchesQuery(r, "  ")).toBe(true);
  });
});

describe("priorNote", () => {
  const missing = (prior_missing: Fleet["prior_missing"]) => ({
    ...fleet([], false),
    prior_missing
  });

  it("says a read failed only when one did", () => {
    expect(priorNote(missing("read_failed"))).toMatch(/^Couldn’t read/);
    expect(priorNote(missing("retention"))).toMatch(/past data retention/);
    expect(priorNote(missing("incomparable"))).toMatch(/count different things/);
    for (const why of ["retention", "incomparable"] as const) {
      expect(priorNote(missing(why))).not.toMatch(/Couldn’t read/);
    }
  });
});
