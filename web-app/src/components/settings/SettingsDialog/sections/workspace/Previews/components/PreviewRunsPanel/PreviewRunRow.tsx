import { ChevronDown, ChevronRight } from "lucide-react";
import { useState } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { dateAgo, parseUtcTimestamp } from "@/libs/utils/date";
import type { PreviewRunKind, PreviewRunSummary } from "@/types/workspace";
import PreviewRunDetailView from "./PreviewRunDetailView";
import PreviewRunStateBadge from "./PreviewRunStateBadge";

const KIND_LABEL: Record<PreviewRunKind, string> = {
  procedure: "Procedure",
  transform_build: "Transform build",
  compare: "Compare",
  airway_sample: "Airway sample"
};

/**
 * One run summary, expandable in place to its step-by-step detail (and, for a
 * `transform_build` or `compare`, its compare result). `childRuns` are a
 * build's linked compare(s) — see `groupPreviewRuns` — rendered nested and
 * indented underneath, each its own expandable row.
 */
export default function PreviewRunRow({
  workspaceId,
  run,
  childRuns = []
}: {
  workspaceId: string;
  run: PreviewRunSummary;
  childRuns?: PreviewRunSummary[];
}) {
  const [open, setOpen] = useState(false);
  const created = parseUtcTimestamp(run.created_at);
  const testId = `preview-run-${run.run_id}`;

  return (
    <div className='flex flex-col gap-1.5' data-testid={testId}>
      <button
        type='button'
        className='flex w-full flex-wrap items-center gap-2 rounded-md border p-2 text-left text-sm hover:bg-muted/50'
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        data-testid={`${testId}-toggle`}
      >
        {open ? (
          <ChevronDown className='size-3.5 shrink-0' />
        ) : (
          <ChevronRight className='size-3.5 shrink-0' />
        )}
        <span className='truncate font-mono'>{run.target_ref}</span>
        {run.kind !== "procedure" && <Badge variant='outline'>{KIND_LABEL[run.kind]}</Badge>}
        <PreviewRunStateBadge state={run.state} outcome={run.outcome} />
        {/* A compare or airway_sample never holds writes (steps: [],
            held_count: 0) — the count adds nothing there, so it's shown
            only where it can be nonzero. */}
        {run.kind !== "compare" && run.kind !== "airway_sample" && (
          <span className='text-muted-foreground text-xs'>{run.held_count} writes held</span>
        )}
        {created && (
          <span className='ml-auto text-muted-foreground text-xs' title={created.toLocaleString()}>
            {dateAgo(created)}
          </span>
        )}
      </button>
      {open && (
        <div className='pl-5'>
          <PreviewRunDetailView workspaceId={workspaceId} runId={run.run_id} />
        </div>
      )}
      {childRuns.length > 0 && (
        <div className='flex flex-col gap-1.5 pl-5' data-testid={`${testId}-children`}>
          {childRuns.map((child) => (
            <PreviewRunRow key={child.run_id} workspaceId={workspaceId} run={child} />
          ))}
        </div>
      )}
    </div>
  );
}
