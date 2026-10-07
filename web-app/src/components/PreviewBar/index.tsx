import { CircleCheck, ExternalLink, Eye } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { usePreviewPin } from "@/contexts/PreviewPinContext";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import type { Workspace } from "@/types/workspace";
import { PreviewRowState } from "./components/PreviewRowState";
import { useIsRevisionServed, useKnownCompareUrl } from "./usePreviewConfirmation";

/**
 * The bar that says "you are looking at a preview".
 *
 * A preview is a mode of the normal product, so nothing else on the page looks
 * different — this bar is the only thing telling the person that what they see
 * is a compiled revision of a branch, not what their team sees. It is therefore
 * solid, full-width, above every workspace page (IDE included), and
 * undismissable: the one way to make it go away is **Exit preview**, which also
 * ends the mode. On a live page it renders nothing at all.
 */
export function PreviewBar() {
  const { status, revisionId, branch, sha, exit } = usePreviewPin();
  const { workspace } = useCurrentWorkspace();
  if (status !== "ready" || !revisionId || !branch || !workspace) return null;
  return (
    <PinnedPreviewBar
      workspace={workspace}
      revisionId={revisionId}
      branch={branch}
      sha={sha}
      onExit={exit}
    />
  );
}

/** The short form of a commit, as git shows it. */
const shortSha = (sha: string) => sha.slice(0, 7);

function PinnedPreviewBar({
  workspace,
  revisionId,
  branch,
  sha,
  onExit
}: {
  workspace: Workspace;
  revisionId: string;
  branch: string;
  sha: string | null;
  onExit: () => void;
}) {
  const compareUrl = useKnownCompareUrl(workspace.id, { branch, sha }, workspace.default_branch);
  const served = useIsRevisionServed(revisionId);

  return (
    <div
      role='status'
      aria-label={`Preview of ${branch}`}
      data-testid='preview-bar'
      data-confirmed={served ? "true" : "false"}
      className='flex min-h-9 w-full shrink-0 flex-wrap items-center gap-x-3 gap-y-1 bg-info px-3 py-1 text-info-foreground text-sm'
    >
      <Eye className='size-4 shrink-0' aria-hidden />
      <p className='flex min-w-0 flex-1 flex-wrap items-center gap-x-2'>
        <span className='font-semibold'>Preview · real data, read-only</span>
        <span className='min-w-0 truncate font-mono' data-testid='preview-bar-branch'>
          {branch}
        </span>
        {sha && (
          <span className='font-mono opacity-80' title={sha} data-testid='preview-bar-sha'>
            @ {shortSha(sha)}
          </span>
        )}
        {served ? (
          <span
            className='inline-flex items-center gap-1 opacity-90'
            title={`The server confirmed it served this page from revision ${revisionId}`}
            data-testid='preview-bar-confirmed'
          >
            <CircleCheck className='size-3.5' aria-hidden />
            Served from this revision
          </span>
        ) : (
          <span className='opacity-80' data-testid='preview-bar-confirmed'>
            Not yet confirmed by the server
          </span>
        )}
        <PreviewRowState workspaceId={workspace.id} branch={branch} revisionId={revisionId} />
      </p>
      {compareUrl && (
        <a
          href={compareUrl}
          target='_blank'
          rel='noopener noreferrer'
          data-testid='preview-bar-changes'
          className='inline-flex items-center gap-1 underline underline-offset-2 hover:opacity-90'
        >
          What changed
          <ExternalLink className='size-3.5' aria-hidden />
        </a>
      )}
      <Button
        size='sm'
        variant='secondary'
        className='h-7'
        onClick={onExit}
        data-testid='preview-bar-exit'
      >
        Exit preview
      </Button>
    </div>
  );
}
