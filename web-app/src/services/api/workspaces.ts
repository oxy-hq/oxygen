import type { WorkspaceMember } from "@/types/organization";
import type { Workspace } from "@/types/workspace";
import { apiClient } from "./axios";

/**
 * What the git half of a workspace looks like when the ide is unreachable.
 *
 * Typed as an exact `Pick` on purpose. Written inline as an object literal it
 * was silently wrong — five capability fields missing and one invented — and
 * `tsc` did not object, because a spread into an object literal does not have
 * to be exhaustive. The annotation is what makes the compiler check it.
 *
 * These values are what the old `/details` returned when it degraded, so
 * nothing downstream sees a new shape.
 */
const GIT_STATE_UNAVAILABLE: Pick<
  Workspace,
  | "active_branch"
  | "detached_head"
  | "git_mode"
  | "capabilities"
  | "default_branch"
  | "protected_branches"
> = {
  active_branch: null,
  detached_head: null,
  git_mode: "none",
  capabilities: {
    can_commit: false,
    can_browse_history: false,
    can_reset_to_commit: false,
    can_switch_branch: false,
    can_diff: false,
    can_push: false,
    can_pull: false,
    can_fetch: false,
    can_force_push: false,
    can_rebase: false,
    can_open_pr: false,
    auto_feature_branch_on_protected: false
  },
  default_branch: "",
  protected_branches: []
};

export const WorkspaceService = {
  /**
   * The workspace: its `/meta` row, which any replica serves from Postgres.
   * There is no working copy and so no git half; the git fields keep the
   * shape a degraded git half used to give, with the repository's default
   * branch from `/meta`.
   */
  async getWorkspace(workspaceId: string): Promise<Workspace> {
    const meta = await apiClient.get(`/${workspaceId}/meta`);
    return {
      ...meta.data,
      ...GIT_STATE_UNAVAILABLE,
      default_branch: meta.data?.default_branch ?? ""
    };
  },

  async listAllWorkspaces(orgId: string): Promise<WorkspaceSummary[]> {
    const response = await apiClient.get(`/orgs/${orgId}/workspaces`);
    return response.data;
  },

  async deleteWorkspace(orgId: string, workspaceId: string, deleteFiles = false): Promise<void> {
    await apiClient.delete(`/orgs/${orgId}/workspaces/${workspaceId}`, {
      params: { delete_files: deleteFiles }
    });
  },

  async renameWorkspace(orgId: string, workspaceId: string, name: string): Promise<void> {
    await apiClient.patch(`/orgs/${orgId}/workspaces/${workspaceId}/rename`, { name });
  },

  async getWorkspaceMembers(workspaceId: string): Promise<WorkspaceMember[]> {
    const response = await apiClient.get<WorkspaceMember[]>(`/${workspaceId}/members`);
    return response.data;
  },

  async setWorkspaceRoleOverride(workspaceId: string, userId: string, role: string): Promise<void> {
    await apiClient.put(`/${workspaceId}/members/${userId}`, { role });
  },

  async removeWorkspaceRoleOverride(workspaceId: string, userId: string): Promise<void> {
    await apiClient.delete(`/${workspaceId}/members/${userId}`);
  }
};

export type WorkspaceStatus = "ready" | "cloning" | "failed" | "not_oxy_project";

export interface WorkspaceSummary {
  id: string;
  org_id: string | null;
  name: string;
  path: string | null;
  created_at: string;
  last_opened_at: string | null;
  created_by_name: string | null;
  status: WorkspaceStatus;
  error: string | null;
  /**
   * `null` when the serving instance holds no working copy for the workspace
   * and therefore did not count. Distinct from `0`, which means it counted and
   * found none — rendering the two the same is how a healthy workspace came to
   * read "0 agents · 0 automations" on a stateless replica.
   */
  agent_count: number | null;
  workflow_count: number | null;
  git_remote: string | null;
  git_commit: string | null;
  git_updated_at: string | null;
}
