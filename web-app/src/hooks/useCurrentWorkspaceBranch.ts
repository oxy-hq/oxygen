import { usePreviewPin } from "@/contexts/PreviewPinContext";
import { useIDE } from "@/pages/ide";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import useIdeBranch from "@/stores/useIdeBranch";
import type { GitCapabilities } from "@/types/workspace";

const NO_GIT_CAPABILITIES: GitCapabilities = {
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
};

export default function useCurrentWorkspaceBranch() {
  const { workspace } = useCurrentWorkspace();
  const { insideIDE } = useIDE();
  const { branch: previewBranch } = usePreviewPin();

  const active_branch = workspace?.active_branch?.name ?? "";

  const { getCurrentBranch } = useIdeBranch();
  const ideBranch = (workspace ? getCurrentBranch(workspace.id) : undefined) ?? active_branch;

  const selectedBranch = insideIDE ? ideBranch : active_branch;

  const defaultBranch = workspace?.default_branch ?? "main";
  const protectedBranches = workspace?.protected_branches ?? [defaultBranch];
  const capabilities = workspace?.capabilities ?? NO_GIT_CAPABILITIES;

  // When the workspace is connected to a remote and the user is on a protected
  // branch, edits are allowed but saving auto-creates a feature branch.
  const isMainEditMode = !!(
    capabilities.auto_feature_branch_on_protected &&
    selectedBranch &&
    protectedBranches.includes(selectedBranch)
  );

  // Git UI is enabled whenever there is a local repo (commit/history/branches work).
  const gitEnabled = capabilities.can_commit;

  return {
    // Non-null assertion: this hook is only used inside authenticated/workspace-scoped routes
    // where workspace is guaranteed to be set. The null case is handled at the route level.
    workspace: workspace!,
    // Only the IDE needs a branch. Everywhere else `selectedBranch` is
    // `active_branch` — the branch the working copy happens to be checked out
    // on — and sending it makes `escalate_for_branch` route the request to the
    // ide singleton, even when the value is "main". That cancels the compile
    // boundary for /apps, /agents, /databases and friends in normal operation.
    //
    // Empty, not `undefined`: that is the server's existing contract for "no
    // branch" — `normalize_branch_hint` filters it to `None`
    // (`normalize_branch_hint_strips_empty` pins it) and `escalate_for_branch`
    // tests `!v.is_empty()`. It also keeps this a `string`, so the ~179
    // consumers do not each have to handle an optional.
    //
    // The server then reads the promoted revision, which is what every surface
    // outside the IDE should see: a working copy parked on a feature branch is
    // the IDE's business, and on a stateless replica the boundary is the only
    // answer there is.
    //
    // A preview pin (`?preview=<revision_id>`, see `PreviewPinContext`)
    // overrides both, inside the IDE and out: while pinned, every surface asks
    // for the preview's branch LABEL. What the server serves is decided by the
    // `x-oxy-preview-revision` header that rides along; the label is here
    // because this value is also part of most query keys, so preview data is
    // cached apart from live data.
    branchName: previewBranch ?? (insideIDE ? selectedBranch : ""),
    /** The pinned preview's branch label, or null when live. */
    previewBranch,
    capabilities,
    isMainEditMode,
    gitEnabled
  };
}
