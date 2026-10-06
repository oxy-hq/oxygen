import { apiClient } from "./axios";

export type WorkspaceHealthStatus = "healthy" | "degraded" | "unhealthy";

export type WorkspaceHealthDimensionKey =
  | "job_liveness"
  | "pipeline"
  | "queue"
  | "reconciliation"
  | "smoke_test"
  | "custom_app_availability";

export interface WorkspaceHealthDimension {
  dimension: WorkspaceHealthDimensionKey;
  status: WorkspaceHealthStatus;
  reason: string | null;
}

/**
 * One reconciliation drift check: an `actual` operand compared against an
 * `expected` reference (each a semantic query, SQL, external source, or constant).
 * Numeric fields are `null` when the source was unreachable or errored (the backend
 * stores NaN/Infinity there, which serialize to JSON null).
 */
export interface WorkspaceHealthReconciliationCheck {
  check: string;
  /** Friendly check-level text; null when the check omitted `description`. */
  description: string | null;
  /** Backend-resolved label for the actual operand ("Actual" when unset). */
  actual_label: string;
  /** Backend-resolved label for the expected operand ("Expected" when unset). */
  expected_label: string;
  actual: number | null;
  expected: number | null;
  abs_diff: number | null;
  /** Percent drift relative to the expected (reference) value, in percent units (3.0 == 3%). */
  pct_diff: number | null;
  status: WorkspaceHealthStatus;
  reason: string | null;
  /**
   * Inclusive `YYYY-MM-DD` window both operands were compared over, already
   * resolved through the check's `freshness` / `timezone` / `offset`. Null on
   * rows stored before the window was recorded.
   */
  window_start: string | null;
  window_end: string | null;
  /** IANA calendar the window was resolved on ("UTC" when the check set none). */
  window_timezone: string | null;
}

/** Which probe produced a smoke check. */
export type WorkspaceHealthSmokeProbeKind = "connection" | "semantic" | "app" | "agent";

/**
 * Whether a probe kind is turned on for the workspace. Sent so the tab can
 * distinguish a *disabled* probe (render "not enabled") from an *enabled* one
 * that produced no verdicts because it found no targets or hasn't run yet — both
 * are otherwise just an absence of checks.
 */
export interface WorkspaceHealthSmokeProbe {
  kind: WorkspaceHealthSmokeProbeKind;
  enabled: boolean;
}

/**
 * One smoke-probe outcome. A `healthy` check that still carries a `reason` is a
 * note, not a problem — the backend uses those to record targets skipped by the
 * `max_targets` cap, so a truncated sweep never reads as full coverage.
 */
export interface WorkspaceHealthSmokeCheck {
  /** Stable id, `"<kind>:<target>"` — e.g. `"connection:bigquery"`. */
  check: string;
  kind: WorkspaceHealthSmokeProbeKind;
  /** What was probed: a database name, topic, app file, or agent ref. */
  target: string;
  status: WorkspaceHealthStatus;
  reason: string | null;
  /** Probe wall time; 0 for checks that never ran a probe (cap notes). */
  duration_ms: number;
}

export interface WorkspaceHealthSignals {
  failed_runs: number;
  timed_out_runs: number;
  total_runs: number;
  airway_last_run_failed: boolean;
  airway_completed_with_errors: boolean;
  open_high_anomalies: number;
  open_medium_anomalies: number;
  dead_letter_count: number;
}

export interface WorkspaceHealthEntry {
  workspace_id: string;
  /** Workspace display name; null only if the workspace row was deleted mid-scan. */
  workspace_name: string | null;
  /** Owning org name; null when the workspace has no org (or was deleted). */
  org_name: string | null;
  status: WorkspaceHealthStatus;
  reasons: string[];
  dimensions: WorkspaceHealthDimension[];
  /**
   * Raw signal counts. `null` right after the payload migration backfills NULL
   * (before the next sweep writes a full payload) — render a fallback, never
   * index into it unguarded.
   */
  signals: WorkspaceHealthSignals | null;
  /** Per-check reconciliation drift detail; empty when the workspace has no `reconcile.yml`. */
  reconciliation: WorkspaceHealthReconciliationCheck[];
  /** Per-probe smoke-test detail; empty when the workspace configures no `health_check.smoke_test`. */
  smoke: WorkspaceHealthSmokeCheck[];
  /**
   * Enabled/disabled state of every smoke probe kind. Empty when the smoke test
   * is disabled entirely, or on rows written before this field existed. When
   * present it always lists all four kinds, so the tab can name a disabled one.
   */
  smoke_probes: WorkspaceHealthSmokeProbe[];
  /**
   * ISO timestamp of when the smoke probes last ran; null when no smoke test is
   * configured. The probes run on their own slower cadence, so these checks are
   * usually older than `checked_at` — render this, not `checked_at`, beside them.
   */
  last_smoke_at: string | null;
  /** ISO timestamp of the last status transition; null until the first eval pass records it. */
  changed_at: string | null;
  /** ISO timestamp of the last eval-pass sweep ("last checked"), refreshed every pass even when status is unchanged; null until the first pass records it. */
  checked_at: string | null;
}

export interface WorkspaceHealthResponse {
  workspaces: WorkspaceHealthEntry[];
}

/**
 * Response to an on-demand eval trigger. The eval is enqueued onto the worker
 * fleet (HTTP 202) rather than run inline, so the trigger returns only the
 * `run_id`; the refreshed row is read back from the rollup once the pass lands.
 */
export interface TriggerEvalResponse {
  run_id: string;
}

/** One change of a workspace's status. */
export interface WorkspaceHealthTransition {
  at: string;
  /** `null` for the first evaluation, and for a state that began before
   *  history was kept. */
  from_status: WorkspaceHealthStatus | null;
  to_status: WorkspaceHealthStatus;
  /** The dimensions failing at that moment. A name the console no longer
   *  knows (a retired dimension) is still listed, as written. */
  failures: { dimension: string; status: string }[];
}

export interface WorkspaceHealthHistory {
  window_days: number;
  /** Changes inside the window, newest first. */
  transitions: WorkspaceHealthTransition[];
  /** The last change before the window: the state it opened in. `null` when
   *  nothing was recorded before the window, and when `truncated`. */
  opening: WorkspaceHealthTransition | null;
  /** More changes happened in the window than are listed. */
  truncated: boolean;
}

export const WorkspaceHealthService = {
  /** A workspace's status changes over the last `days` (at most 90 are kept). */
  async history(workspaceId: string, days: number): Promise<WorkspaceHealthHistory> {
    const response = await apiClient.get<WorkspaceHealthHistory>(
      `/admin/workspace-health/${workspaceId}/history`,
      { params: { days } }
    );
    return response.data;
  },

  async list(): Promise<WorkspaceHealthResponse> {
    const response = await apiClient.get<WorkspaceHealthResponse>("/admin/workspace-health");
    return response.data;
  },
  /**
   * Enqueue an eval pass for one workspace. `smoke: true` additionally forces the
   * workspace's smoke probes to run on this pass even if their (default 6h)
   * cadence has not elapsed — the "Run smoke test" button. Without it the pass
   * refreshes the passive Postgres signals and reuses the last smoke verdicts.
   *
   * It forces the cadence, not the config: a workspace with
   * `smoke_test: { enabled: false }` runs no probes either way.
   */
  async trigger(workspaceId: string, smoke = false): Promise<TriggerEvalResponse> {
    const response = await apiClient.post<TriggerEvalResponse>(
      `/admin/workspace-health/${workspaceId}/eval`,
      null,
      { params: smoke ? { smoke: true } : undefined }
    );
    return response.data;
  }
};
