import { ChevronDown, ChevronRight } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { usePreviewChecks } from "@/hooks/api/workspaces/usePreviews";
import { cn } from "@/libs/shadcn/utils";
import type { PreviewChecksSummary } from "@/types/workspace";
import PreviewPipelineCheckRow from "./PreviewPipelineCheckRow";
import PreviewTransformCheckRow from "./PreviewTransformCheckRow";
import {
  previewChecksExpandable,
  previewChecksSummaryText,
  previewChecksSummaryTone
} from "./summary";

interface Props {
  workspaceId: string;
  branch: string;
  /** The row's embedded summary — cheap, always present; drives the label. */
  summary: PreviewChecksSummary | null;
  testId: string;
}

const TONE_CLASS: Record<string, string> = {
  pending: "text-muted-foreground",
  muted: "text-muted-foreground",
  warning: "text-warning",
  danger: "text-destructive"
};

/**
 * A preview row's Airway-checks verdict: the cheap summary is always shown;
 * clicking it lazily fetches and expands the per-pipeline detail (only once
 * there's something to expand — a `pending`/`null` summary has none yet).
 */
export default function PreviewChecksPanel({ workspaceId, branch, summary, testId }: Props) {
  const [open, setOpen] = useState(false);
  const expandable = previewChecksExpandable(summary);
  const detail = usePreviewChecks(workspaceId, branch, open && expandable);

  const label = previewChecksSummaryText(summary);
  const tone = previewChecksSummaryTone(summary);

  const trigger = (
    <Button
      variant='ghost'
      size='sm'
      className={cn("h-6 gap-1 px-1.5 text-xs", TONE_CLASS[tone])}
      onClick={() => setOpen((v) => !v)}
      disabled={!expandable}
      aria-expanded={open}
      data-testid={`${testId}-checks-toggle`}
    >
      {expandable &&
        (open ? <ChevronDown className='size-3' /> : <ChevronRight className='size-3' />)}
      {label}
    </Button>
  );

  if (!open || !expandable) return trigger;

  return (
    <div className='flex flex-col gap-2'>
      {trigger}
      <div className='flex flex-col gap-2 pl-1' data-testid={`${testId}-checks-detail`}>
        {detail.isLoading && <Spinner className='size-4 text-muted-foreground' />}
        {detail.error && <p className='text-destructive text-xs'>{detail.error.message}</p>}
        {detail.data?.status === "failed" && (
          <p className='text-destructive text-xs'>
            {detail.data.error ?? "The check analysis failed."}
          </p>
        )}
        {detail.data &&
          detail.data.pipelines.length === 0 &&
          detail.data.transforms.length === 0 &&
          detail.data.status === "done" && (
            <p className='text-muted-foreground text-xs'>
              No pipeline or transform changes on this branch.
            </p>
          )}
        {detail.data?.pipelines.map((pipeline) => (
          <PreviewPipelineCheckRow key={pipeline.file_path} pipeline={pipeline} />
        ))}
        {detail.data && detail.data.transforms.length > 0 && (
          <div className='flex flex-col gap-2' data-testid={`${testId}-transforms`}>
            <p className='font-medium text-muted-foreground text-xs uppercase tracking-wide'>
              Transforms
            </p>
            {detail.data.transforms.map((transform) => (
              <PreviewTransformCheckRow
                key={transform.file_path}
                workspaceId={workspaceId}
                transform={transform}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
