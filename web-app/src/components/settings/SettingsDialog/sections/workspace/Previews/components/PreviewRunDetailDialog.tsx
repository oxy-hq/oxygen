import type { ReactNode } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger
} from "@/components/ui/shadcn/dialog";
import PreviewRunDetailView from "./PreviewRunsPanel/PreviewRunDetailView";

interface Props {
  workspaceId: string;
  runId: string;
  title: string;
  trigger: ReactNode;
  testId: string;
}

/**
 * A run's detail (steps, and a compare once queued) behind a dialog rather
 * than a route — this feature has none, it all lives in the Settings dialog.
 * Reused wherever something outside the Runs panel needs to point at a run
 * without lifting that panel's open/selected state up to share it — today,
 * the Checks panel's link from a transform to its queued build.
 */
export default function PreviewRunDetailDialog({
  workspaceId,
  runId,
  title,
  trigger,
  testId
}: Props) {
  return (
    <Dialog>
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent className='max-h-[80vh] overflow-y-auto' data-testid={testId}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
        </DialogHeader>
        <PreviewRunDetailView workspaceId={workspaceId} runId={runId} />
      </DialogContent>
    </Dialog>
  );
}
