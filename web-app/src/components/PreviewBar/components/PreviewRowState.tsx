import { RefreshCw, TriangleAlert } from "lucide-react";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { usePreviewPin } from "@/contexts/PreviewPinContext";
import { usePreview } from "@/hooks/api/workspaces/usePreviews";
import useSettingsDialog from "@/stores/useSettingsDialog";

/**
 * What has happened to this branch's preview since the pinned revision
 * compiled. The pinned revision is immutable and keeps serving; this only says
 * whether there is something newer — so the person isn't reviewing an old
 * compile without knowing it.
 */
export function PreviewRowState({
  workspaceId,
  branch,
  revisionId
}: {
  workspaceId: string;
  branch: string;
  revisionId: string;
}) {
  const { preview } = usePreview(workspaceId, branch);
  const { enter } = usePreviewPin();
  const openSettings = useSettingsDialog((s) => s.open);
  const toPreviews = () => openSettings("workspace.previews");

  if (!preview) return null;

  if (preview.status === "ready" && preview.revision_id && preview.revision_id !== revisionId) {
    const newer = preview.revision_id;
    return (
      <button
        type='button'
        onClick={() => enter({ revisionId: newer, branch, sha: preview.sha })}
        data-testid='preview-bar-state'
        className='inline-flex items-center gap-1 font-medium underline underline-offset-2'
      >
        <RefreshCw className='size-3.5' aria-hidden />A newer compile is ready — open it
      </button>
    );
  }
  if (preview.status === "compiling") {
    return (
      <span className='inline-flex items-center gap-1 opacity-90' data-testid='preview-bar-state'>
        <Spinner className='size-3' />
        Recompiling…
      </span>
    );
  }
  if (preview.status === "stale" || preview.status === "failed") {
    return (
      <button
        type='button'
        onClick={toPreviews}
        data-testid='preview-bar-state'
        className='inline-flex items-center gap-1 font-medium underline underline-offset-2'
      >
        <TriangleAlert className='size-3.5' aria-hidden />
        {preview.status === "stale"
          ? "The branch has moved on — refresh in Previews"
          : "The latest compile failed — see Previews"}
      </button>
    );
  }
  return null;
}
