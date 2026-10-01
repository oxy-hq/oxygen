import { Badge } from "@/components/ui/shadcn/badge";
import { cn } from "@/libs/shadcn/utils";
import type { PreviewPipelineCheck } from "@/types/workspace";
import PreviewVerdictBadge from "../PreviewVerdictBadge";

const CHANGE_LABEL: Record<PreviewPipelineCheck["change"], string> = {
  added: "Added",
  modified: "Modified",
  removed: "Removed"
};

/**
 * One pipeline's verdict plus its findings. A `needs_reset` finding names the
 * prod action up front — that is the one thing an operator must act on before
 * this branch can ship — everything else (kind, detail) is supporting detail.
 */
export default function PreviewPipelineCheckRow({ pipeline }: { pipeline: PreviewPipelineCheck }) {
  return (
    <div
      className='flex flex-col gap-2 rounded-md border p-3'
      data-testid={`preview-pipeline-check-${pipeline.name}`}
    >
      <div className='flex flex-wrap items-center gap-2'>
        <span className='font-medium font-mono text-sm'>{pipeline.name}</span>
        <Badge variant='outline'>{CHANGE_LABEL[pipeline.change]}</Badge>
        <PreviewVerdictBadge verdict={pipeline.verdict} />
        <span className='ml-auto truncate text-muted-foreground text-xs' title={pipeline.file_path}>
          {pipeline.file_path}
        </span>
      </div>
      {pipeline.findings.length > 0 && (
        <ul className='flex flex-col gap-2'>
          {pipeline.findings.map((finding, i) => (
            // Findings carry no id of their own; `kind` repeats per pipeline
            // at most rarely, so pairing it with position keeps the key stable.
            // biome-ignore lint/suspicious/noArrayIndexKey: findings have no stable id
            <li key={`${finding.kind}-${i}`} className='flex flex-col gap-1 border-t pt-2 text-sm'>
              <div className='flex flex-wrap items-center gap-2'>
                <PreviewVerdictBadge verdict={finding.verdict} />
                <span className='text-muted-foreground text-xs'>{finding.kind}</span>
              </div>
              <p>{finding.detail}</p>
              {/* `null` for an `additive` finding — nothing to do, so nothing to show. */}
              {finding.prod_action && (
                <p
                  className={cn(
                    "text-xs",
                    finding.verdict === "needs_reset"
                      ? "font-medium text-destructive"
                      : "text-muted-foreground"
                  )}
                >
                  Prod action: {finding.prod_action}
                </p>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
