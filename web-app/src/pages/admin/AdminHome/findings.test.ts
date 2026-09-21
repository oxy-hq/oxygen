import { describe, expect, it } from "vitest";
import type { WorkspaceCompileRow } from "@/services/api/compiles";
import type { QueueStatusCounts } from "@/services/api/internalJobs";
import type { WorkspaceHealthEntry } from "@/services/api/workspaceHealth";
import type { AppHealth, AppHealthRow, FleetHealthResponse } from "@/types/apps";
import { appsReport, compilesReport, healthReport, jobsReport, rankFindings } from "./findings";

/**
 * These decide whether the console says "you are needed". Three failure modes matter more
 * than the copy:
 *
 * - **crying wolf** — reporting a quiet queue or an unmeasured app as a problem, which
 *   teaches operators to skim past the page;
 * - **false calm** — a source that *failed to load* reading as healthy. This one shipped
 *   and was caught in review: every report fell through to its base "all clear" sentence
 *   on `undefined`, and the home rendered it under a green tick. The test meant to catch
 *   it asserted `ok` did not match `/All \d/` — which the base sentence satisfies while
 *   still claiming health. It passed for the wrong reason;
 * - **over-claiming** — speaking for a whole fleet from one page of it.
 */

const ws = (status: WorkspaceHealthEntry["status"], reasons: string[]): WorkspaceHealthEntry => ({
  workspace_id: `w-${Math.random()}`,
  workspace_name: "W",
  org_name: "O",
  status,
  reasons,
  dimensions: [],
  signals: null,
  reconciliation: [],
  smoke: [],
  smoke_probes: [],
  last_smoke_at: null,
  changed_at: null,
  checked_at: null
});
const health = (...workspaces: WorkspaceHealthEntry[]) => ({ data: { workspaces } });

const counts = (over: Partial<QueueStatusCounts> = {}): QueueStatusCounts => ({
  queued: 0,
  claimed: 0,
  completed: 0,
  failed: 0,
  cancelled: 0,
  dead: 0,
  ...over
});
const stats = (last24h: Partial<QueueStatusCounts> = {}) => ({
  data: { last_1h: counts(), last_24h: counts(last24h), total: counts() }
});
const noWorkers = { data: undefined };
const fleetOf = (...workers: Array<{ worker_id: string; last_claim_at: string | null }>) => ({
  data: { supported: true, workers: workers.map((w) => ({ ...w, inflight_count: 0 })) }
});

const row = (over: Partial<WorkspaceCompileRow> = {}): WorkspaceCompileRow => ({
  workspace_id: "w",
  workspace_name: "W",
  workspace_path: null,
  current_revision_id: "r1",
  current_status: "ready",
  current_git_sha: null,
  latest_revision_id: "r1",
  latest_status: "ready",
  latest_started_at: null,
  last_ready_at: null,
  revision_count: 1,
  ready_count: 1,
  failed_count: 0,
  current_is_latest_ready: true,
  ...over
});
const compiles = (...rows: WorkspaceCompileRow[]) => ({ data: { rows } });

const app = (h: AppHealth, reason: string | null = null): AppHealthRow =>
  ({
    app_id: `a-${h}-${Math.random()}`,
    app_slug: "s",
    app_name: "A",
    org_id: "o",
    org_slug: "o",
    health: h,
    reason,
    requests: 0,
    failed: 0,
    baseline: null,
    window_minutes: 60
  }) as AppHealthRow;
const fleet = (apps: AppHealthRow[], configured = true) => ({
  data: {
    apps,
    total: apps.length,
    has_more: false,
    evaluated_at: "",
    observability_configured: configured
  } as FleetHealthResponse
});

describe("a source that failed to load", () => {
  /**
   * A control plane that is down must never render under the same green tick as a control
   * plane with nothing to report — on the page whose whole job is to answer *does
   * anything need me?*.
   */
  const cases = [
    ["workspace health", () => healthReport({ data: undefined, isError: true })],
    ["the job queue", () => jobsReport({ data: undefined, isError: true }, noWorkers)],
    ["compile revisions", () => compilesReport({ data: undefined, isError: true })],
    ["custom-app health", () => appsReport({ data: undefined, isError: true })]
  ] as const;

  for (const [what, run] of cases) {
    it(`says it could not check ${what}, rather than claiming health`, () => {
      const r = run();
      expect(r.okTone).toBe("unknown");
      expect(r.ok).toBe(`Couldn't check ${what}.`);
      expect(r.findings).toEqual([]);
    });
  }

  it("treats a resolved-but-empty response as unanswered too", () => {
    expect(healthReport({ data: undefined }).okTone).toBe("unknown");
  });

  it("calls the queue unknown when only the worker half failed", () => {
    expect(jobsReport(stats(), { data: undefined, isError: true }).okTone).toBe("unknown");
  });
});

describe("healthReport", () => {
  it("states the number of causes, not just the number of workspaces", () => {
    const same = "one shared failure";
    const r = healthReport(health(ws("unhealthy", [same]), ws("unhealthy", [same])));
    expect(r.findings[0].title).toContain("2 workspaces");
    expect(r.findings[0].detail).toBe("from 1 distinct cause");
  });

  it("is quiet when every workspace is healthy, and says how many it checked", () => {
    const r = healthReport(health(ws("healthy", []), ws("healthy", [])));
    expect(r.findings).toEqual([]);
    expect(r.ok).toBe("All 2 workspaces healthy.");
    expect(r.okTone).toBeUndefined();
  });

  it("reports degraded-only as a warning, not a failure", () => {
    expect(healthReport(health(ws("degraded", ["x"]))).findings[0].tone).toBe("warn");
  });
});

describe("jobsReport", () => {
  it("calls dead jobs a failure and merely failed ones a warning", () => {
    const r = jobsReport(stats({ dead: 2, failed: 5 }), noWorkers);
    expect(r.findings.map((f) => [f.id, f.tone])).toEqual([
      ["jobs-dead", "danger"],
      ["jobs-failed", "warn"]
    ]);
  });

  it("reads the 24h window, not all-time totals", () => {
    const r = jobsReport(
      { data: { last_1h: counts(), last_24h: counts(), total: counts({ dead: 99 }) } },
      noWorkers
    );
    expect(r.findings).toEqual([]);
  });

  it("flags a worker that has stopped claiming", () => {
    const r = jobsReport(
      stats(),
      fleetOf(
        { worker_id: "a", last_claim_at: new Date().toISOString() },
        { worker_id: "b", last_claim_at: new Date(Date.now() - 3_600_000).toISOString() }
      )
    );
    const stale = r.findings.find((f) => f.id === "jobs-stale-workers");
    expect(stale?.tone).toBe("danger");
    expect(stale?.detail).toBe("of 2 in the fleet");
  });

  it("says so plainly when the entire fleet has gone stale", () => {
    const r = jobsReport(stats(), fleetOf({ worker_id: "a", last_claim_at: null }));
    expect(r.findings[0].detail).toBe("the whole fleet has stopped claiming");
  });

  it("does not cry wolf over an idle worker on a quiet queue", () => {
    const r = jobsReport(
      stats(),
      fleetOf({ worker_id: "a", last_claim_at: new Date(Date.now() - 120_000).toISOString() })
    );
    expect(r.findings).toEqual([]);
  });

  it("treats an unsupported deployment as a missing capability, not an outage", () => {
    const r = jobsReport(stats(), { data: { supported: false, workers: [] } });
    expect(r.findings).toEqual([]);
    expect(r.okTone).toBeUndefined();
  });
});

describe("compilesReport", () => {
  it("flags workspaces with no promoted revision as serving 503s", () => {
    const r = compilesReport(compiles(row({ current_status: "pending" }), row()));
    expect(r.findings[0].id).toBe("compiles-not-ready");
    expect(r.findings[0].tone).toBe("danger");
  });

  it("flags a workspace serving an older revision as a warning, not a failure", () => {
    const r = compilesReport(compiles(row({ current_is_latest_ready: false })));
    expect(r.findings.map((f) => [f.id, f.tone])).toEqual([["compiles-behind", "warn"]]);
  });

  it("is quiet when everything is ready and current", () => {
    const r = compilesReport(compiles(row(), row()));
    expect(r.findings).toEqual([]);
    expect(r.ok).toBe("All 2 workspaces compiled and up to date.");
  });

  it("does not speak for the whole fleet when the page came back full", () => {
    // A full page may be a truncated fleet: "All 2 workspaces compiled" would be a claim
    // about workspaces this request never saw.
    expect(compilesReport(compiles(row(), row()), 2).ok).toBe(
      "The 2 workspaces checked are compiled and up to date."
    );
  });

  it("still speaks plainly when the page came back short of the limit", () => {
    expect(compilesReport(compiles(row()), 2).ok).toBe("All 1 workspace compiled and up to date.");
  });
});

describe("appsReport", () => {
  it("flags a down app and quotes its reason", () => {
    const r = appsReport(fleet([app("down", "manifest invalid")]));
    expect(r.findings[0].tone).toBe("danger");
    expect(r.findings[0].detail).toBe("manifest invalid");
  });

  it("does not treat an app with no traffic as a problem", () => {
    expect(appsReport(fleet([app("quiet"), app("operational")])).findings).toEqual([]);
  });

  it("says capture is off rather than calling unmeasured apps healthy", () => {
    const r = appsReport(fleet([app("not_measured"), app("not_measured")], false));
    expect(r.findings).toEqual([]);
    expect(r.ok).toContain("observability capture is not configured");
    expect(r.okTone).toBe("muted");
  });

  it("leaves a genuinely clear source unqualified", () => {
    expect(appsReport(fleet([app("operational")])).okTone).toBeUndefined();
    expect(compilesReport(compiles(row())).okTone).toBeUndefined();
  });
});

describe("rankFindings", () => {
  it("puts every failure above every warning, across sources", () => {
    const ranked = rankFindings([
      compilesReport(compiles(row({ current_is_latest_ready: false }))),
      jobsReport(stats({ dead: 1 }), noWorkers)
    ]);
    expect(ranked.map((f) => f.tone)).toEqual(["danger", "warn"]);
  });

  it("carries the source through so a finding can be attributed", () => {
    const ranked = rankFindings([jobsReport(stats({ dead: 1 }), noWorkers)]);
    expect(ranked[0].source.label).toBe("Internal jobs");
  });

  it("returns nothing when every source is clear", () => {
    expect(
      rankFindings([compilesReport(compiles(row())), appsReport(fleet([app("operational")]))])
    ).toEqual([]);
  });

  it("does not turn an unanswered source into a finding", () => {
    // It belongs in the "could not be checked" line, not the attention list — a source
    // that is down is not a fleet problem to triage.
    expect(rankFindings([healthReport({ data: undefined, isError: true })])).toEqual([]);
  });
});
