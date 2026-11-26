import { CheckCircle2, XCircle } from "lucide-react";
import { Badge } from "@/components/ui/shadcn/badge";
import { cn } from "@/libs/shadcn/utils";
import type { AirwayResourceVerdict } from "@/services/api/airwayConfig";
import { splitPipelineRef } from "../../utils";

/**
 * One verdict bucket (Passing / Failing) in the preview's resources-grouped-
 * by-verdict layout. Emerald is reserved for workflow-node success, so
 * "passing" uses the `status-success` token, not `emerald`/`green`.
 */
export function VerdictGroup({
  title,
  count,
  tone,
  resources
}: {
  title: string;
  count: number;
  tone: "pass" | "fail";
  resources: AirwayResourceVerdict[];
}) {
  return (
    <div>
      <div
        className={cn(
          "mb-1.5 flex items-center gap-1.5 text-[10px] uppercase tracking-[0.14em]",
          tone === "pass" ? "text-status-success-text" : "text-status-error-text"
        )}
      >
        {tone === "pass" ? <CheckCircle2 className='size-3' /> : <XCircle className='size-3' />}
        {title} ({count})
      </div>
      <ul className='space-y-1'>
        {resources.map((r) => (
          <ResourceRow key={`${r.pipeline_ref}:${r.resource}`} resource={r} />
        ))}
      </ul>
    </div>
  );
}

function ResourceRow({ resource }: { resource: AirwayResourceVerdict }) {
  const { path, workspaceId } = splitPipelineRef(resource.pipeline_ref);
  return (
    <li className=' border border-border/60 px-2.5 py-1.5 text-xs'>
      <div className='flex items-center justify-between gap-2'>
        <span className='font-mono'>{path}</span>
        <Badge variant='outline' className='shrink-0 text-[10px]'>
          {resource.mutability}
        </Badge>
      </div>
      <div className='mt-0.5 flex items-center justify-between gap-2 text-muted-foreground'>
        <span className='truncate'>
          {resource.resource}
          {resource.reason ? ` — ${resource.reason}` : ""}
        </span>
        <span className='shrink-0 font-mono text-[10px]'>{workspaceId}</span>
      </div>
    </li>
  );
}
