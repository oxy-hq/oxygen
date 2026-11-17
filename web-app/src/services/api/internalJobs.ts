import { apiClient } from "./axios";

export interface QueueStatusCounts {
  queued: number;
  claimed: number;
  completed: number;
  failed: number;
  cancelled: number;
  dead: number;
}

export interface QueueStatsResponse {
  last_1h: QueueStatusCounts;
  last_24h: QueueStatusCounts;
  total: QueueStatusCounts;
}

export interface QueueRow {
  task_id: string;
  run_id: string;
  queue_status: string;
  worker_id: string | null;
  claim_count: number;
  max_claims: number;
  last_heartbeat: string | null;
  claimed_at: string | null;
  created_at: string;
  updated_at: string;
  task_type: string | null;
  /** Decoded TaskSpec JSON (agent_id / workflow_ref / question / variables…). */
  spec: unknown;
  // Enriched tenant + run context (LEFT-joined; null for system/orphan jobs).
  workspace_id: string | null;
  workspace_name: string | null;
  org_id: string | null;
  org_name: string | null;
  run_status: string | null;
  run_error_message: string | null;
  originating_user_email: string | null;
}

export interface DeadLetterResponse {
  rows: QueueRow[];
  total: number;
}

export interface WorkerDto {
  worker_id: string;
  last_claim_at: string | null;
  inflight_count: number;
}

export interface WorkersResponse {
  supported: boolean;
  workers: WorkerDto[];
}

export interface ScheduledJob {
  name: string;
  interval_secs: number;
  description: string;
  last_known_run_at: string | null;
  trigger_path: string | null;
}

/**
 * One cause of failure: failed and dead jobs of one task type, in one workspace, whose
 * errors match once ids and counts are masked (`GET /admin/internal-jobs/failure-groups`).
 */
export interface FailureGroup {
  task_type: string;
  workspace_id: string | null;
  workspace_name: string | null;
  org_id: string | null;
  org_name: string | null;
  /** The error with ids and counts masked — what the group is keyed on. */
  signature: string;
  /** The newest occurrence's message, unmasked. */
  latest_error: string | null;
  occurrences: number;
  dead: number;
  first_seen: string;
  last_seen: string;
  latest_task_id: string;
}

export interface FailureGroupsResponse {
  days: number;
  groups: FailureGroup[];
}

export interface RunReaperResponse {
  rows_affected: number;
}

const BASE = "/admin/internal-jobs";

export const InternalJobsService = {
  async queueStats(): Promise<QueueStatsResponse> {
    const response = await apiClient.get<QueueStatsResponse>(`${BASE}/queue-stats`);
    return response.data;
  },

  async failureGroups(days: number): Promise<FailureGroupsResponse> {
    const response = await apiClient.get<FailureGroupsResponse>(`${BASE}/failure-groups`, {
      params: { days }
    });
    return response.data;
  },
  async recentFailures(limit: number = 50): Promise<QueueRow[]> {
    const response = await apiClient.get<QueueRow[]>(`${BASE}/recent-failures`, {
      params: { limit }
    });
    return response.data;
  },

  async deadLetter(limit: number = 50, offset: number = 0): Promise<DeadLetterResponse> {
    const response = await apiClient.get<DeadLetterResponse>(`${BASE}/dead-letter`, {
      params: { limit, offset }
    });
    return response.data;
  },

  async reenqueueDead(taskId: string): Promise<QueueRow> {
    const response = await apiClient.post<QueueRow>(
      `${BASE}/dead-letter/${encodeURIComponent(taskId)}/reenqueue`
    );
    return response.data;
  },

  async deleteDead(taskId: string): Promise<void> {
    await apiClient.delete(`${BASE}/dead-letter/${encodeURIComponent(taskId)}`);
  },

  async workers(): Promise<WorkersResponse> {
    const response = await apiClient.get<WorkersResponse>(`${BASE}/workers`);
    return response.data;
  },

  async scheduled(): Promise<ScheduledJob[]> {
    const response = await apiClient.get<ScheduledJob[]>(`${BASE}/scheduled`);
    return response.data;
  },

  async runReaper(): Promise<RunReaperResponse> {
    const response = await apiClient.post<RunReaperResponse>(`${BASE}/run-reaper`);
    return response.data;
  },

  /**
   * Trigger any scheduled job by its registry `trigger_path`. The backend
   * returns the full path including the `/api` prefix; the axios client already
   * has `/api` in its baseURL, so strip it before posting.
   */
  async runScheduled(
    triggerPath: string
  ): Promise<{ rows_affected?: number; rows_deleted?: number }> {
    const path = triggerPath.replace(/^\/api(?=\/)/, "");
    const response = await apiClient.post<{ rows_affected?: number; rows_deleted?: number }>(path);
    return response.data;
  }
};
