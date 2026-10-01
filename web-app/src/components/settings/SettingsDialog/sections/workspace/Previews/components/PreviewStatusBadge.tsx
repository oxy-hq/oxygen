import { Badge } from "@/components/ui/shadcn/badge";
import { Spinner } from "@/components/ui/shadcn/spinner";
import type { WorkspacePreviewStatus } from "@/types/workspace";

export default function PreviewStatusBadge({ status }: { status: WorkspacePreviewStatus }) {
  if (status === "compiling") {
    return (
      <Badge variant='secondary' data-testid='preview-status'>
        <Spinner className='size-3' />
        Compiling
      </Badge>
    );
  }
  if (status === "failed") {
    return (
      <Badge variant='destructive' data-testid='preview-status'>
        Failed
      </Badge>
    );
  }
  if (status === "stale") {
    return (
      <Badge variant='outline' data-testid='preview-status'>
        Stale
      </Badge>
    );
  }
  return (
    <Badge variant='default' data-testid='preview-status'>
      Ready
    </Badge>
  );
}
