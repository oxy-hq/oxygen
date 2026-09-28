import { apiBaseURL } from "../env";
import { apiClient } from "./axios";

export interface ActiveRunEntry {
  run_id: string;
  status: "running" | "suspended" | "done" | "failed" | "cancelled";
  question: string;
  agent_id: string;
  source_type: string;
  attempt: number;
  /** Set when this run was seeded by a scheduler fire (or `run_now`). */
  schedule_id?: string | null;
  /** "scheduled" | "manual" | "backfill" | "retry" | "promote"; null for legacy runs. */
  trigger?: string | null;
  /** Active runs aren't anomaly-enriched in v1 — slot reserved for symmetry. */
  anomaly?: AnomalyInfo | null;
  /** Active runs aren't cost-enriched (run still in flight); slot reserved
   *  so `NormalizedRun.costUsd` can come from a shared `buildRun()`. */
  cost_usd?: number | null;
  /** Same symmetry slot for token totals — populated by run history, not
   *  the active-runs endpoint. */
  tokens_total?: number | null;
  created_at: string;
  updated_at: string;
}

export interface ActiveRunsResponse {
  runs: ActiveRunEntry[];
  total: number;
}

export interface AnomalyInfo {
  /** "duration_spike" today; cost/row buckets land with per-type metrics. */
  kind: string;
  /** Human summary, e.g. "12m 23s vs p50=4m 11s". */
  detail: string;
  /** "warning" (≥ 2× baseline) or "critical" (≥ 5× baseline). */
  severity: "warning" | "critical" | string;
}

export interface RunHistoryEntry {
  run_id: string;
  status: string;
  question: string;
  agent_id: string;
  source_type: string;
  answer?: string;
  error_message?: string;
  attempt: number;
  /** Set when this run was seeded by a scheduler fire (or `run_now`). */
  schedule_id?: string | null;
  /** "scheduled" | "manual" | "backfill" | "retry" | "promote"; null for legacy runs. */
  trigger?: string | null;
  /** Server-side heuristic flag — "healthy but weird" runs. */
  anomaly?: AnomalyInfo | null;
  /** Estimated USD cost of this run's LLM calls — derived from token
   *  counts × per-million pricing, not persisted at write time. `null`
   *  for non-LLM runs and for runs whose every model is missing from
   *  the pricing table. */
  cost_usd?: number | null;
  /** Total tokens (input + output + cache writes + cache reads).
   *  Surfaced even when `cost_usd` is null so a model missing from the
   *  pricing table still shows raw usage. `null` for non-LLM runs. */
  tokens_total?: number | null;
  created_at: string;
  updated_at: string;
}

export interface RunHistoryResponse {
  runs: RunHistoryEntry[];
  total: number;
}

export interface RunEventEntry {
  seq: number;
  event_type: string;
  payload: Record<string, unknown>;
}

export interface LlmUsage {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens: number;
  cache_read_input_tokens: number;
  /** Distinct models seen across this run's LLM calls. */
  models: string[];
  /** Number of completed LLM HTTP rounds. */
  call_count: number;
  /** USD; null when no model on the run is in the pricing table. */
  cost_usd?: number | null;
}

export interface DagStepSummary {
  step_name: string;
  /** "succeeded" | "failed" | "cached" | "running" */
  status: string;
  started_at: string;
  /** null while in flight. */
  completed_at?: string | null;
  /** Wall-clock ms; null while in flight. */
  duration_ms?: number | null;
  error?: string | null;
  cached: boolean;
}

export interface EltTableSummary {
  table_name: string;
  /** Rows pulled from the source connector. */
  rows_extracted?: number | null;
  /** Rows landed on the destination. */
  rows_loaded?: number | null;
  /** "pending" | "extracting" | "extracted" | "loading" | "loaded" | "failed" */
  status: string;
  extract_started_at?: string | null;
  extract_completed_at?: string | null;
  loaded_at?: string | null;
}

export interface TaskTreeNode {
  run_id: string;
  parent_run_id: string | null;
  status: string;
  question: string;
  agent_id: string;
  source_type: string;
  answer?: string;
  error_message?: string;
  attempt: number;
  task_status?: string;
  /** "scheduled" | "manual" | "backfill" | "retry" | "promote"; null for legacy runs. */
  trigger?: string | null;
  created_at: string;
  updated_at: string;
  outcome_status?: string;
  event_log?: RunEventEntry[];
  /** Populated by the backend on the root node only — agent runs' total
   *  token / cost summary. */
  llm_usage?: LlmUsage | null;
  /** Populated on automation (DAG) runs' root only — per-step timings. */
  dag_steps?: DagStepSummary[] | null;
  /** Populated on airway (ELT) runs' root only — per-table row counts. */
  elt_tables?: EltTableSummary[] | null;
  /** Airway pipeline lineage labels stamped on the run at start time
   *  so the UI can render Source / Destination cards even before the
   *  `pipeline_plan` event arrives. `null` for non-airway runs. */
  source_kind?: string | null;
  destination_label?: string | null;
  pipeline_name?: string | null;
  /** File path that authored this run — `metadata.pipeline_ref` for
   *  airway, `metadata.workflow_ref` for automation. Drives the
   *  "Edit YAML" link from a run detail back to the IDE file editor.
   *  `null` for runs that don't have a YAML source. */
  source_ref?: string | null;
}

export interface TaskTreeResponse {
  root_id: string;
  nodes: TaskTreeNode[];
}

export interface AgentStats {
  agent_id: string;
  total: number;
  succeeded: number;
  failed: number;
  recovered: number;
}

export interface RecoveredRunEntry {
  run_id: string;
  status: string;
  question: string;
  agent_id: string;
  attempt: number;
  created_at: string;
  updated_at: string;
}

export interface RecoveryResponse {
  total_runs: number;
  recovered_count: number;
  failed_count: number;
  cancelled_count: number;
  succeeded_count: number;
  agents: AgentStats[];
  recovered_runs: RecoveredRunEntry[];
}

export interface QueueTaskEntry {
  task_id: string;
  run_id: string;
  queue_status: string;
  worker_id?: string;
  claim_count: number;
  max_claims: number;
  last_heartbeat?: string;
  created_at: string;
  updated_at: string;
}

export interface QueueHealthResponse {
  queued: number;
  claimed: number;
  completed: number;
  failed: number;
  cancelled: number;
  dead: number;
  stale_tasks: QueueTaskEntry[];
  dead_tasks: QueueTaskEntry[];
}

export class CoordinatorService {
  static async getActiveRuns(
    projectId: string,
    params: { include_system?: boolean } = {}
  ): Promise<ActiveRunsResponse> {
    const response = await apiClient.get(`/${projectId}/analytics/coordinator/active-runs`, {
      params
    });
    return response.data;
  }

  static async getRunHistory(
    projectId: string,
    params: {
      limit?: number;
      offset?: number;
      status?: string;
      source_type?: string;
      /** Narrow to runs seeded by a specific schedule. */
      schedule_id?: string;
      /** Include system-managed daemons (preagg_cycle, etc.). Default off. */
      include_system?: boolean;
    } = {}
  ): Promise<RunHistoryResponse> {
    const response = await apiClient.get(`/${projectId}/analytics/coordinator/runs`, {
      params: { limit: params.limit ?? 25, ...params }
    });
    return response.data;
  }

  static async getRunTree(projectId: string, runId: string): Promise<TaskTreeResponse> {
    const response = await apiClient.get(`/${projectId}/analytics/coordinator/runs/${runId}/tree`);
    return response.data;
  }

  static async getRecoveryStats(projectId: string, limit = 200): Promise<RecoveryResponse> {
    const response = await apiClient.get(`/${projectId}/analytics/coordinator/recovery`, {
      params: { limit }
    });
    return response.data;
  }

  static async getQueueHealth(projectId: string): Promise<QueueHealthResponse> {
    const response = await apiClient.get(`/${projectId}/analytics/coordinator/queue`);
    return response.data;
  }

  static async retryRun(projectId: string, runId: string): Promise<{ run_id: string }> {
    const response = await apiClient.post(
      `/${projectId}/analytics/coordinator/runs/${runId}/retry`
    );
    return response.data;
  }

  static liveStreamUrl(projectId: string): string {
    return `${apiBaseURL}/${projectId}/analytics/coordinator/live`;
  }
}
