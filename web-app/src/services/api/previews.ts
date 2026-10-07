import type {
  PreviewChecksResponse,
  PreviewRunDetail,
  PreviewRunSummary,
  PreviewSourceItem,
  StartPreviewRunRequest,
  StartPreviewRunResponse,
  UpsertPreviewSourceRequest,
  WorkspacePreview,
  WorkspacePreviewItemResponse,
  WorkspacePreviewListResponse
} from "@/types/workspace";
import { apiClient } from "./axios";

/**
 * The ONE place the previews route is spelled. Workspace-scoped, like every
 * other per-workspace surface (`/api/{workspace_id}/…`, mounted under the
 * `/{workspace_id}` nest in `router/protected.rs`).
 */
const previewsPath = (workspaceId: string) => `/${workspaceId}/previews`;
const base = previewsPath;

/** `GET …/previews/checks` — the per-pipeline Airway change analysis. */
const previewChecksPath = (workspaceId: string) => `${base(workspaceId)}/checks`;

/** `GET/POST …/previews/runs[/…]` — held procedure dry-runs on a preview. */
const previewRunsPath = (workspaceId: string) => `${base(workspaceId)}/runs`;

/**
 * `GET/PUT …/previews/sources` — sandbox credentials for a rotate-on-use
 * pipeline (QuickBooks). Workspace-scoped, not branch-scoped: one sandbox
 * company per pipeline serves every branch's Airway sample of it. Not gated
 * by `OXY_PREVIEW_RUNS`.
 */
const previewSourcesPath = (workspaceId: string) => `${base(workspaceId)}/sources`;

/**
 * Workspace previews: a branch compiled so the product can be opened on it
 * without it being live.
 *
 * None of these calls carry `?branch=` as a pin — the branch is the resource
 * being managed, not the revision the request is served from — so the list
 * reads the same whether or not the page is itself pinned to a preview.
 * Refresh and delete name the branch in the query string because a branch
 * name contains `/`, which a path segment cannot carry unencoded.
 */
export const PreviewService = {
  async list(workspaceId: string): Promise<WorkspacePreview[]> {
    const res = await apiClient.get<WorkspacePreviewListResponse>(base(workspaceId));
    return res.data.items;
  },

  async create(workspaceId: string, branch: string): Promise<WorkspacePreview> {
    const res = await apiClient.post<WorkspacePreviewItemResponse>(base(workspaceId), { branch });
    return res.data.item;
  },

  async refresh(workspaceId: string, branch: string): Promise<WorkspacePreview> {
    const res = await apiClient.post<WorkspacePreviewItemResponse>(
      `${base(workspaceId)}/refresh`,
      undefined,
      { params: { branch } }
    );
    return res.data.item;
  },

  async remove(workspaceId: string, branch: string): Promise<void> {
    await apiClient.delete(base(workspaceId), { params: { branch } });
  },

  /** The per-pipeline Airway change analysis for a branch's current revision. */
  async checks(workspaceId: string, branch: string): Promise<PreviewChecksResponse> {
    const res = await apiClient.get<PreviewChecksResponse>(previewChecksPath(workspaceId), {
      params: { branch }
    });
    return res.data;
  },

  /** Start a held dry-run of a procedure on a preview's staging revision. */
  async startRun(
    workspaceId: string,
    request: StartPreviewRunRequest
  ): Promise<StartPreviewRunResponse> {
    const res = await apiClient.post<StartPreviewRunResponse>(
      previewRunsPath(workspaceId),
      request
    );
    return res.data;
  },

  /** Runs for one branch, newest first — capped server-side at 50. */
  async listRuns(workspaceId: string, branch: string): Promise<PreviewRunSummary[]> {
    const res = await apiClient.get<PreviewRunSummary[]>(previewRunsPath(workspaceId), {
      params: { branch }
    });
    return res.data;
  },

  async getRun(workspaceId: string, runId: string): Promise<PreviewRunDetail> {
    const res = await apiClient.get<PreviewRunDetail>(`${previewRunsPath(workspaceId)}/${runId}`);
    return res.data;
  },

  /** Registered sandbox sources for the workspace, by pipeline. */
  async listSources(workspaceId: string): Promise<PreviewSourceItem[]> {
    const res = await apiClient.get<PreviewSourceItem[]>(previewSourcesPath(workspaceId));
    return res.data;
  },

  /** Register or edit a pipeline's sandbox source (upsert by pipeline name). */
  async upsertSource(
    workspaceId: string,
    request: UpsertPreviewSourceRequest
  ): Promise<PreviewSourceItem> {
    const res = await apiClient.put<PreviewSourceItem>(previewSourcesPath(workspaceId), request);
    return res.data;
  }
};
