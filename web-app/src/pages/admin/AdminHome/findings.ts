import ROUTES from "@/libs/utils/routes";
import { groupByCause } from "@/pages/admin/AdminWorkspaceHealth/groupByCause";
import type { AdminTone } from "@/pages/admin/components/adminTone";
import { workerLiveness } from "@/pages/admin/components/workerLiveness";
import type { WorkspaceCompileRow } from "@/services/api/compiles";
import type { QueueStatsResponse, WorkersResponse } from "@/services/api/internalJobs";
import type { WorkspaceHealthResponse } from "@/services/api/workspaceHealth";
import type { FleetHealthResponse } from "@/types/apps";

/**
 * The console's landing page asks one question — *does anything need me?* — and these
 * functions answer it, one signal source at a time.
 *
 * Each returns a report: the findings worth waking someone for, and the sentence to show
 * when there are none. Keeping them pure and separate from the page is what lets the
 * ranking be tested against real fleet shapes instead of eyeballed in a browser; the
 * thresholds below are the ones the detail pages already use, imported rather than
 * restated.
 */
export type Finding = {
  /** Stable across renders and unique within a report — React key and `data-testid`. */
  id: string;
  tone: Extract<AdminTone, "danger" | "warn">;
  /** The headline, with the number in it: "9 workspaces unhealthy". */
  title: string;
  /** The qualifier that decides whether to click: "from 1 distinct cause". */
  detail: string;
  /** Where to go to act on it. */
  to: string;
};

/**
 * What a report is given: the query, not its data. Passing `data` alone made a failed
 * fetch indistinguishable from a healthy fleet — every report fell through to its "all
 * clear" sentence and the home rendered it under a green tick, on the one page whose job
 * is to answer *does anything need me?*. Caught in review of #3256.
 */
export type SourceInput<T> = { data: T | undefined; isError?: boolean };

export type SourceReport = {
  key: string;
  label: string;
  to: string;
  findings: Finding[];
  /** What to say when `findings` is empty. Never "OK" alone — say what was checked. */
  ok: string;
  /**
   * Whether `ok` is a clean bill of health, a caveat, or no answer at all.
   *
   * - `muted` — nothing is down, but nothing was measured either ("capture is not
   *   configured"); a tick would claim a verdict nobody took.
   * - `unknown` — the source itself failed to load. Never a tick: a control plane that
   *   is down must not read as a control plane with nothing to report.
   */
  okTone?: "ok" | "muted" | "unknown";
};

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** The shared "this source did not answer" outcome. */
const unanswered = (base: Omit<SourceReport, "findings">, what: string): SourceReport => ({
  ...base,
  findings: [],
  ok: `Couldn't check ${what}.`,
  okTone: "unknown"
});

export function healthReport(query: SourceInput<WorkspaceHealthResponse>): SourceReport {
  const base = {
    key: "health",
    label: "Workspace health",
    to: ROUTES.ADMIN.WORKSPACE_HEALTH,
    ok: "No workspace is failing its health checks."
  };
  if (query.isError || !query.data) return unanswered(base, "workspace health");
  const data = query.data;

  const attention = data.workspaces.filter((w) => w.status !== "healthy");
  if (attention.length === 0) {
    return {
      ...base,
      findings: [],
      ok: `All ${plural(data.workspaces.length, "workspace")} healthy.`
    };
  }
  const causes = groupByCause(attention);
  const unhealthy = attention.filter((w) => w.status === "unhealthy").length;
  return {
    ...base,
    findings: [
      {
        id: "health-attention",
        tone: unhealthy > 0 ? "danger" : "warn",
        title: `${plural(attention.length, "workspace")} need${attention.length === 1 ? "s" : ""} attention`,
        // The number that changes what you do: one cause is one fix, nine is an outage.
        detail: `from ${plural(causes.length, "distinct cause")}`,
        to: ROUTES.ADMIN.WORKSPACE_HEALTH
      }
    ]
  };
}

export function jobsReport(
  statsQuery: SourceInput<QueueStatsResponse>,
  workersQuery: SourceInput<WorkersResponse>
): SourceReport {
  const base = {
    key: "jobs",
    label: "Internal jobs",
    to: ROUTES.ADMIN.INTERNAL_JOBS,
    ok: "Queue is clear and every worker is answering."
  };
  // Either half failing means the queue's state is unknown: a clear queue with an
  // unreachable worker list is not "every worker is answering".
  if (statsQuery.isError || workersQuery.isError || (!statsQuery.data && !workersQuery.data)) {
    return unanswered(base, "the job queue");
  }
  const stats = statsQuery.data;
  const workers = workersQuery.data;
  const findings: Finding[] = [];

  if (stats) {
    // The 24h window, not `total`: a job that died last month is history, not an alert.
    const { dead, failed } = stats.last_24h;
    if (dead > 0) {
      findings.push({
        id: "jobs-dead",
        tone: "danger",
        title: `${plural(dead, "dead job")} in 24h`,
        detail: "exhausted every retry — re-enqueue or discard",
        to: ROUTES.ADMIN.INTERNAL_JOBS
      });
    }
    if (failed > 0) {
      findings.push({
        id: "jobs-failed",
        tone: "warn",
        title: `${plural(failed, "failed job")} in 24h`,
        detail: "retryable, but check the failure is not systematic",
        to: ROUTES.ADMIN.INTERNAL_JOBS
      });
    }
  }

  // `supported: false` means the deployment's schema predates the column — that is a
  // missing capability, not a broken fleet, so it must not read as an alert.
  if (workers?.supported) {
    const stale = workers.workers.filter((w) => workerLiveness(w.last_claim_at) === "stale");
    if (stale.length > 0) {
      findings.push({
        id: "jobs-stale-workers",
        tone: "danger",
        title: `${plural(stale.length, "worker")} stale`,
        detail:
          stale.length === workers.workers.length
            ? "the whole fleet has stopped claiming"
            : `of ${workers.workers.length} in the fleet`,
        to: ROUTES.ADMIN.INTERNAL_JOBS
      });
    }
  }

  const ok =
    workers?.supported && workers.workers.length > 0
      ? `Queue clear; ${plural(workers.workers.length, "worker")} answering.`
      : base.ok;
  return { ...base, findings, ok };
}

export function compilesReport(
  query: SourceInput<{ rows: WorkspaceCompileRow[] }>,
  /** What the page asked for. A full page may be a truncated fleet — say so. */
  limit?: number
): SourceReport {
  const base = {
    key: "compiles",
    label: "Compile revisions",
    to: ROUTES.ADMIN.COMPILES,
    ok: "Every workspace is compiled and serving its latest revision."
  };
  if (query.isError || !query.data) return unanswered(base, "compile revisions");
  const rows = query.data.rows;

  const findings: Finding[] = [];
  const notReady = rows.filter((r) => r.current_status !== "ready");
  const behind = rows.filter((r) => r.current_status === "ready" && !r.current_is_latest_ready);

  if (notReady.length > 0) {
    findings.push({
      id: "compiles-not-ready",
      tone: "danger",
      title: `${plural(notReady.length, "workspace")} not serving`,
      // This is the 503 `needs_recompile` an operator gets paged about.
      detail: "no promoted revision — these answer 503",
      to: ROUTES.ADMIN.COMPILES
    });
  }
  if (behind.length > 0) {
    findings.push({
      id: "compiles-behind",
      tone: "warn",
      title: `${plural(behind.length, "workspace")} behind`,
      detail: "serving an older revision than the latest ready one",
      to: ROUTES.ADMIN.COMPILES
    });
  }
  // `rows.length === limit` means the page was full, so there may be more behind it —
  // "All 200 workspaces compiled" would be a claim about a fleet this never saw.
  const truncated = limit !== undefined && rows.length >= limit;
  return {
    ...base,
    findings,
    ok: truncated
      ? `The ${plural(rows.length, "workspace")} checked are compiled and up to date.`
      : `All ${plural(rows.length, "workspace")} compiled and up to date.`
  };
}

export function appsReport(query: SourceInput<FleetHealthResponse>): SourceReport {
  const base = {
    key: "apps",
    label: "Custom apps",
    to: ROUTES.ADMIN.CUSTOMER_APPS,
    ok: "Every published app is serving."
  };
  if (query.isError || !query.data) return unanswered(base, "custom-app health");
  const data = query.data;

  const findings: Finding[] = [];
  const down = data.apps.filter((a) => a.health === "down");
  const degraded = data.apps.filter((a) => a.health === "degraded");

  if (down.length > 0) {
    findings.push({
      id: "apps-down",
      tone: "danger",
      title: `${plural(down.length, "app")} down`,
      detail: down[0].reason ?? "check the app's build and manifest",
      to: ROUTES.ADMIN.CUSTOMER_APPS
    });
  }
  if (degraded.length > 0) {
    findings.push({
      id: "apps-degraded",
      tone: "warn",
      title: `${plural(degraded.length, "app")} degraded`,
      detail: degraded[0].reason ?? "elevated failures against its own baseline",
      to: ROUTES.ADMIN.CUSTOMER_APPS
    });
  }

  // Capture being off is a *console* gap, not an app outage: every app then reads
  // "unmeasured", which is exactly the state that must not be mistaken for healthy.
  if (!data.observability_configured) {
    return {
      ...base,
      findings,
      ok: "No app is being measured — observability capture is not configured.",
      okTone: "muted"
    };
  }
  // `quiet` (no traffic) and `not_measured` are deliberately not findings: neither is an
  // outage, and treating "nobody visited" as a problem is how a console teaches its
  // operators to ignore it.
  return { ...base, findings, ok: `All ${plural(data.apps.length, "published app")} serving.` };
}

const TONE_RANK: Record<Finding["tone"], number> = { danger: 0, warn: 1 };

/** Every finding across the reports, worst first. */
export function rankFindings(reports: SourceReport[]): Array<Finding & { source: SourceReport }> {
  return reports
    .flatMap((source) => source.findings.map((f) => ({ ...f, source })))
    .sort((a, b) => TONE_RANK[a.tone] - TONE_RANK[b.tone]);
}
