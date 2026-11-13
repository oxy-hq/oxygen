import { usePreviewPin } from "@/contexts/PreviewPinContext";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * The current workspace and the branch label its requests carry.
 *
 * There is no branch to select: the workspace is edited in its repository and
 * every surface reads the revision the workspace serves, so `branchName` is ""
 * — the server's contract for "no branch" (`normalize_branch_hint` filters it
 * to `None`).
 *
 * A preview pin (`?preview=<revision_id>`, see `PreviewPinContext`) is the one
 * exception: while pinned, every surface asks for the preview's branch LABEL.
 * What the server serves is decided by the `x-oxy-preview-revision` header that
 * rides along; the label is here because this value is also part of most query
 * keys, so preview data is cached apart from live data.
 */
export default function useCurrentWorkspaceBranch() {
  const { workspace } = useCurrentWorkspace();
  const { branch: previewBranch } = usePreviewPin();

  return {
    // Non-null assertion: this hook is only used inside authenticated/workspace-scoped routes
    // where workspace is guaranteed to be set. The null case is handled at the route level.
    workspace: workspace!,
    branchName: previewBranch ?? "",
    /** The pinned preview's branch label, or null when live. */
    previewBranch
  };
}
