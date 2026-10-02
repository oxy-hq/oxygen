import type { ProjectStatus } from "@/types/github";
import type { WorkspaceMember } from "@/types/organization";
import type { RevisionInfo } from "@/types/settings";
import type { Workspace, WorkspaceBranch, WorkspaceBranchesResponse } from "@/types/workspace";
import { apiClient } from "./axios";

export interface CommitEntry {
  hash: string;
  short_hash: string;
  message: string;
  author: string;
  /** Relative committer date ("9 minutes ago") — matches the order rows arrive in. */
  date: string;
  /**
   * `false` when the commit exists only in the workspace's working copy and has
   * never been pushed. Omitted when there is no upstream to compare against.
   * Local-only commits block fast-forward pulls and make restore refuse, so the
   * list calls them out explicitly.
   */
  on_remote?: boolean;
}

export interface RecentCommitsResponse {
  commits: CommitEntry[];
  has_more: boolean;
}

export interface RecentCommitsParams {
  limit?: number;
  offset?: number;
}

export type DirtyKind = "modified" | "staged" | "untracked" | "deleted" | "conflicted";

export interface DirtyEntry {
  path: string;
  kind: DirtyKind;
}

export interface ResetToCommitResponse {
  success: boolean;
  message: string;
  /** Set when the restore was refused because the working tree is dirty. */
  dirty?: DirtyEntry[];
  /**
   * Set when the restore was refused because it would drop commits. Both
   * refusals are recoverable by re-issuing with `force=true`, so each needs its
   * own confirmation — a refusal with no way to act on it reads as a dead button.
   */
  discarded_commits?: CommitEntry[];
}

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
  async getGithubRevisionInfo(workspaceId: string, branchName: string): Promise<RevisionInfo> {
    const response = await apiClient.get(`/${workspaceId}/revision-info`, {
      params: { branch: branchName }
    });
    return response.data;
  },

  /**
   * The workspace, assembled from its two halves.
   *
   * `/meta` is Postgres-only and served by any replica. `/git-state` needs the
   * ide's `.git` and 502s when the ide is unreachable — which is the point:
   * the workspace page keeps rendering, and the git chrome degrades.
   *
   * A failed git half yields `active_branch: null` and `git_mode: "none"`,
   * exactly what the old `/details` returned when it degraded. Every consumer
   * downstream reads the merged object out of the store, so nothing else
   * changes — including `useWorkspaceStatus`, which is gated on a branch being
   * present and must stay closed when there is none.
   */
  async getWorkspace(workspaceId: string): Promise<Workspace> {
    const [meta, gitState] = await Promise.all([
      apiClient.get(`/${workspaceId}/meta`),
      apiClient.get(`/${workspaceId}/git-state`).catch(() => null)
    ]);
    return {
      ...meta.data,
      ...(gitState?.data ?? GIT_STATE_UNAVAILABLE)
    };
  },

  async getWorkspaceBranches(workspaceId: string): Promise<WorkspaceBranchesResponse> {
    const response = await apiClient.get(`/${workspaceId}/branches`);
    return response.data;
  },

  async getWorkspaceStatus(workspace_id: string, branch_name?: string): Promise<ProjectStatus> {
    const response = await apiClient.get<ProjectStatus>(
      `/${workspace_id}/status`,
      branch_name ? { params: { branch: branch_name } } : undefined
    );
    return response.data;
  },

  async switchWorkspaceBranch(
    workspaceId: string,
    branchName: string,
    baseBranch?: string
  ): Promise<WorkspaceBranch> {
    const response = await apiClient.post(`/${workspaceId}/switch-branch`, {
      branch: branchName,
      ...(baseBranch ? { base_branch: baseBranch } : {})
    });
    return response.data;
  },

  async pullChanges(
    workspaceId: string,
    branchName: string
  ): Promise<{
    success: boolean;
    /**
     * What the pull actually did ("Already up to date with origin", "Pulled 3
     * commits — now at cbe3089"). Surface it verbatim; do not substitute an
     * optimistic string, or a no-op pull reads as a successful sync.
     */
    message: string;
    state: "Synced" | "AheadOfRemote" | "Conflict" | "Error";
  }> {
    const response = await apiClient.post(`/${workspaceId}/pull-changes`, null, {
      params: { branch: branchName }
    });
    return response.data;
  },

  async fetchRemote(
    workspaceId: string,
    branchName: string
  ): Promise<{ success: boolean; message: string }> {
    const response = await apiClient.post(`/${workspaceId}/fetch`, null, {
      params: { branch: branchName }
    });
    return response.data;
  },

  async continueRebase(
    workspaceId: string,
    branchName: string
  ): Promise<{ success: boolean; message: string }> {
    const response = await apiClient.post(`/${workspaceId}/continue-rebase`, null, {
      params: { branch: branchName }
    });
    return response.data;
  },

  async resolveConflictWithContent(
    workspaceId: string,
    branchName: string,
    filePath: string,
    content: string
  ): Promise<{ success: boolean; message: string }> {
    const response = await apiClient.post(
      `/${workspaceId}/resolve-conflict-with-content`,
      { content },
      { params: { branch: branchName, file: filePath } }
    );
    return response.data;
  },

  async unresolveConflictFile(
    workspaceId: string,
    branchName: string,
    filePath: string
  ): Promise<{ success: boolean; message: string }> {
    const response = await apiClient.post(`/${workspaceId}/unresolve-conflict-file`, null, {
      params: { branch: branchName, file: filePath }
    });
    return response.data;
  },

  async resolveConflictFile(
    workspaceId: string,
    branchName: string,
    filePath: string,
    side: "mine" | "theirs"
  ): Promise<{ success: boolean; message: string }> {
    const response = await apiClient.post(`/${workspaceId}/resolve-conflict-file`, null, {
      params: { branch: branchName, file: filePath, side }
    });
    return response.data;
  },

  async abortRebase(
    workspaceId: string,
    branchName: string
  ): Promise<{ success: boolean; message: string }> {
    const response = await apiClient.post(`/${workspaceId}/abort-rebase`, null, {
      params: { branch: branchName }
    });
    return response.data;
  },

  async pushChanges(
    workspaceId: string,
    branchName: string,
    commitMessage?: string
  ): Promise<{ success: boolean; message: string }> {
    const response = await apiClient.post(
      `/${workspaceId}/push-changes`,
      { commit_message: commitMessage },
      { params: { branch: branchName } }
    );
    return response.data;
  },

  async deleteBranch(
    workspaceId: string,
    branchName: string
  ): Promise<{ success: boolean; message: string }> {
    const response = await apiClient.delete(
      `/${workspaceId}/branches/${encodeURIComponent(branchName)}`
    );
    return response.data;
  },

  async forcePushBranch(
    workspaceId: string,
    branchName: string
  ): Promise<{ success: boolean; message: string }> {
    const response = await apiClient.post(`/${workspaceId}/force-push`, null, {
      params: { branch: branchName }
    });
    return response.data;
  },

  async discardAllChanges(
    workspaceId: string,
    branchName: string
  ): Promise<{ success: boolean; message: string }> {
    const response = await apiClient.post(`/${workspaceId}/discard-all`, null, {
      params: { branch: branchName }
    });
    return response.data;
  },

  async getRecentCommits(
    workspaceId: string,
    branchName: string,
    params: RecentCommitsParams = {}
  ): Promise<RecentCommitsResponse> {
    const response = await apiClient.get(`/${workspaceId}/recent-commits`, {
      params: { branch: branchName, ...params }
    });
    return response.data;
  },

  async resetToCommit(
    workspaceId: string,
    branchName: string,
    commit: string,
    force = false
  ): Promise<ResetToCommitResponse> {
    const response = await apiClient.post(`/${workspaceId}/reset-to-commit`, null, {
      params: { branch: branchName, commit, force }
    });
    return response.data;
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
   * read "0 agents · 0 automations · 0 apps" on a stateless replica.
   */
  agent_count: number | null;
  workflow_count: number | null;
  app_count: number | null;
  git_remote: string | null;
  git_commit: string | null;
  git_updated_at: string | null;
}
