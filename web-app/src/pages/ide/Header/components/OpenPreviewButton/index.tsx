import { Eye } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { usePreviewPin } from "@/contexts/PreviewPinContext";
import useCanUsePreviews from "@/hooks/useCanUsePreviews";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { useOpenPreview } from "./useOpenPreview";

interface Props {
  workspaceId: string;
  branch: string;
}

const LABELS = {
  idle: "Open preview",
  requesting: "Starting preview…",
  waiting: "Compiling preview…"
} as const;

/**
 * Beside the IDE's branch picker, on a non-default branch: open the real
 * product on this branch, on real data, without making it live. Staff only,
 * like every preview surface, and hidden while this branch is already the
 * pinned preview (the bar across the top says so, and offers the exit).
 */
export function OpenPreviewButton({ workspaceId, branch }: Props) {
  const canUsePreviews = useCanUsePreviews();
  const { branch: pinned, enter } = usePreviewPin();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const { phase, start, notice } = useOpenPreview(workspaceId, branch, (ready) =>
    enter(
      { revisionId: ready.revision_id, branch: ready.branch, sha: ready.sha },
      ROUTES.ORG(orgSlug).WORKSPACE(workspaceId).HOME
    )
  );

  if (!canUsePreviews || pinned === branch) return null;

  const busy = phase !== "idle";
  return (
    <>
      <Button
        size='sm'
        variant='outline'
        onClick={() => void start()}
        disabled={busy}
        tooltip={busy ? undefined : `Open the product on ${branch} — real data, read-only`}
        data-testid='ide-open-preview'
        data-phase={phase}
      >
        {busy ? <Spinner className='size-3.5' /> : <Eye />}
        {LABELS[phase]}
      </Button>
      {notice && (
        // Why it could not compile — uncommitted edits, no checkout — in the
        // server's words, right where the person pressed.
        <span
          role='alert'
          title={notice}
          className='line-clamp-2 max-w-72 text-destructive text-xs'
          data-testid='ide-open-preview-error'
        >
          {notice}
        </span>
      )}
    </>
  );
}
