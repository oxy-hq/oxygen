import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import type { PreviewTransformChange, PreviewTransformCheck } from "@/types/workspace";
import PreviewRunDetailDialog from "../PreviewRunDetailDialog";

const CHANGE_LABEL: Record<PreviewTransformChange, string> = {
  added: "Added",
  modified: "Modified"
};

/**
 * One changed pure-Airhouse transform: `auto` built itself in the preview and
 * (once queued) compares with live; `manual` needs an operator to run it, and
 * `reason` says why — same "why" slot an `auto` transform uses when it has no
 * `build_run_id` yet (e.g. "builds skipped: …").
 */
export default function PreviewTransformCheckRow({
  workspaceId,
  transform
}: {
  workspaceId: string;
  transform: PreviewTransformCheck;
}) {
  const testId = `preview-transform-check-${transform.name}`;
  return (
    <div className='flex flex-col gap-2 rounded-md border p-3' data-testid={testId}>
      <div className='flex flex-wrap items-center gap-2'>
        <span className='font-medium font-mono text-sm'>{transform.name}</span>
        <Badge variant='outline'>{CHANGE_LABEL[transform.change]}</Badge>
        <Badge variant={transform.build === "auto" ? "default" : "secondary"}>
          {transform.build === "auto" ? "Auto build" : "Manual"}
        </Badge>
        <span
          className='ml-auto truncate text-muted-foreground text-xs'
          title={transform.file_path}
        >
          {transform.file_path}
        </span>
      </div>
      {transform.reason && <p className='text-muted-foreground text-xs'>{transform.reason}</p>}
      {transform.build_run_id && (
        <PreviewRunDetailDialog
          workspaceId={workspaceId}
          runId={transform.build_run_id}
          title={`Build: ${transform.name}`}
          testId={`${testId}-build-dialog`}
          trigger={
            <Button
              variant='link'
              size='sm'
              className='h-6 w-fit px-0 text-xs'
              data-testid={`${testId}-build-link`}
            >
              View build run
            </Button>
          }
        />
      )}
    </div>
  );
}
