import { apiClient } from "./axios";

export interface CompileEnqueueResponse {
  task_id: string;
  workspace_id: string;
  branch: string;
}

export interface RevisionSummary {
  revision_id: string;
  status: string;
  kind: string;
  branch: string | null;
  /** SHA the revision was compiled against. Compared to `head_sha`
   * to label the button as up-to-date vs stale. */
  git_sha: string;
  started_at: string;
  finished_at: string | null;
  duration_ms: number | null;
}

export interface CompileStatus {
  workspace_id: string;
  current_revision_id: string | null;
  latest: RevisionSummary | null;
  can_compile: boolean;
  /**
   * HEAD commit on the workspace's **working copy** default branch. Null for
   * blank / demo / no-remote workspaces.
   *
   * Never decide freshness from this alone: compiles are taken from this same
   * ref, so `latest.git_sha === head_sha` is a tautology after any successful
   * compile and stays true however far origin has moved ahead.
   */
  head_sha: string | null;
  /** SHA of the promoted revision reads are actually served from. This is what
   * "is my change live?" asks about. Null when nothing is promoted yet. */
  compiled_sha: string | null;
  /** SHA of `origin/<default_branch>` as of the last fetch. Null when unknown. */
  remote_sha: string | null;
  /** When origin was last contacted. `remote_sha` is only as trustworthy as
   * this is recent — qualify the verdict when it is stale or null. */
  remote_fetched_at: string | null;
  /**
   * Position of the serving revision relative to `origin/<default_branch>`.
   * `compiled_sha !== remote_sha` alone does not mean "behind" — a revision
   * compiled from a local-only commit is *ahead* and fails the same equality.
   * Null when either end is unknown.
   */
  compiled_ahead: number | null;
  compiled_behind: number | null;
  /**
   * Whether the serving revision was compiled from the commit the working
   * copy's default branch is on now. `false` means "not known to match": it
   * was compiled from another commit, or from a snapshot of the disk that
   * names no commit at all. Null when there is no repository or nothing is
   * promoted; absent from a server that predates the field.
   */
  compiled_matches_head?: boolean | null;
  /** Workspace's default branch (`main` / `master` / custom). Null
   * matches `head_sha = null`. */
  default_branch: string | null;
  /** True only on a multi-instance (split-fleet) deployment where compiling
   * promotes the revision a separate `serve` fleet reads. False on a single
   * `all` instance (e.g. `oxy start` / `oxy serve --local`), which serves from
   * the working copy directly — manual compile is a no-op there, so the IDE
   * hides the Compile button. */
  boundary_active: boolean;
}

export const CompileService = {
  async enqueue(workspaceId: string, branch: string): Promise<CompileEnqueueResponse> {
    const res = await apiClient.post<CompileEnqueueResponse>(`/${workspaceId}/compile`, undefined, {
      params: { branch }
    });
    return res.data;
  },

  async status(workspaceId: string, branch: string): Promise<CompileStatus> {
    const res = await apiClient.get<CompileStatus>(`/${workspaceId}/compile/status`, {
      params: { branch }
    });
    return res.data;
  }
};
