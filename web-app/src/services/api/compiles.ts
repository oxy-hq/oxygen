import { apiClient } from "./axios";

/**
 * One row in the recent-compiles list. Mirrors the backend's
 * `admin::compiles::CompileRow`.
 */
export interface CompileRow {
  revision_id: string;
  workspace_id: string;
  git_sha: string;
  branch: string | null;
  status: "compiling" | "ready" | "failed" | string;
  kind: "main" | "draft" | string;
  owner_user_id: string | null;
  compiler_version: string;
  schema_version: number;
  started_at: string;
  finished_at: string | null;
  duration_ms: number | null;
  file_count_seen: number;
  file_count_compiled: number;
  file_count_failed: number;
  is_current_for_workspace: boolean;
}

interface CompileFailure {
  path: string;
  kind: "yaml" | "io" | "shape" | "duplicate" | string;
  message: string;
}

/** One entity successfully compiled into a revision — the "which compiled" unit. */
export interface CompiledEntity {
  kind: string;
  name: string;
  file_path: string;
}

export interface CompileDetail extends CompileRow {
  error_summary: { failures?: CompileFailure[]; fatal?: string } | null;
  /** Every entity successfully compiled into this revision ("which compiled"). */
  compiled_entities: CompiledEntity[];
}

export interface ListCompilesResponse {
  rows: CompileRow[];
  total_returned: number;
}

/**
 * One aggregated row in the "By workspace" view. Mirrors the backend's
 * `admin::compiles::WorkspaceCompileRow`: a rollup of a workspace's
 * compile history (current vs. latest revision, ready/failed counts)
 * so operators can scan tenants without expanding every revision list.
 */
export interface WorkspaceCompileRow {
  workspace_id: string;
  workspace_name: string | null;
  workspace_path: string | null;
  current_revision_id: string | null;
  current_status: string | null;
  current_git_sha: string | null;
  latest_revision_id: string | null;
  latest_status: string | null;
  latest_started_at: string | null;
  last_ready_at: string | null;
  revision_count: number;
  ready_count: number;
  failed_count: number;
  current_is_latest_ready: boolean;
}

export interface ListWorkspaceCompilesResponse {
  rows: WorkspaceCompileRow[];
  total_returned: number;
}

interface BatchRunResult {
  workspace_id: string;
  task_id: string | null;
  error: string | null;
}

export interface BatchRunResponse {
  enqueued: number;
  results: BatchRunResult[];
}

interface BatchPromoteResult {
  revision_id: string;
  workspace_id: string | null;
  error: string | null;
}

export interface BatchPromoteResponse {
  promoted: number;
  results: BatchPromoteResult[];
}

export interface RunCompileRequest {
  workspace_id: string;
  git_sha?: string;
  branch?: string;
  promote?: boolean;
}

export interface RunCompileResponse {
  task_id: string;
  workspace_id: string;
  promote: boolean;
}

export interface BackfillResponse {
  enqueued: number;
  /** True when the batch cap was hit and more uncompiled workspaces remain. */
  remaining: boolean;
  task_ids: string[];
}

export interface PromoteResponse {
  revision_id: string;
  workspace_id: string;
}

const BASE = "/admin/compiles";

export const CompilesService = {
  async list(params: {
    limit?: number;
    workspace_id?: string;
    org_id?: string;
    status?: string;
  }): Promise<ListCompilesResponse> {
    const res = await apiClient.get<ListCompilesResponse>(BASE, {
      params
    });
    return res.data;
  },

  /** Aggregated "By workspace" rollup — one row per workspace. */
  async listWorkspaces(params: {
    limit?: number;
    offset?: number;
    q?: string;
    status?: string;
  }): Promise<ListWorkspaceCompilesResponse> {
    const res = await apiClient.get<ListWorkspaceCompilesResponse>(`${BASE}/workspaces`, {
      params
    });
    return res.data;
  },

  /** Enqueue a promoting compile for each selected workspace. */
  async batchRun(workspaceIds: string[], promote: boolean): Promise<BatchRunResponse> {
    const res = await apiClient.post<BatchRunResponse>(`${BASE}/batch/run`, {
      workspace_ids: workspaceIds,
      promote
    });
    return res.data;
  },

  /** Repoint each selected revision's workspace at that revision. */
  async batchPromote(revisionIds: string[]): Promise<BatchPromoteResponse> {
    const res = await apiClient.post<BatchPromoteResponse>(`${BASE}/batch/promote`, {
      revision_ids: revisionIds
    });
    return res.data;
  },

  async detail(revisionId: string): Promise<CompileDetail> {
    const res = await apiClient.get<CompileDetail>(`${BASE}/${encodeURIComponent(revisionId)}`);
    return res.data;
  },

  async runNow(request: RunCompileRequest): Promise<RunCompileResponse> {
    const res = await apiClient.post<RunCompileResponse>(`${BASE}/run`, request);
    return res.data;
  },

  /** Enqueue a promoting compile for every workspace that has never been compiled. */
  async backfillUncompiled(): Promise<BackfillResponse> {
    const res = await apiClient.post<BackfillResponse>(`${BASE}/backfill`);
    return res.data;
  },

  /**
   * Rollback lever: repoint the revision's workspace at this (ready, main)
   * revision — used to revert a bad compile to a known-good prior one.
   */
  async promote(revisionId: string): Promise<PromoteResponse> {
    const res = await apiClient.post<PromoteResponse>(
      `${BASE}/${encodeURIComponent(revisionId)}/promote`
    );
    return res.data;
  }
};
