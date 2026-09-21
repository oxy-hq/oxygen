import { ChevronDown, ChevronRight, Loader2, RotateCcw, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { cn } from "@/libs/utils/cn";
import type { QueueRow } from "@/services/api/internalJobs";
import { relativeTime } from "../../../utils";
import { JobDebugPanel } from "./JobDebugPanel";
import { statusTone } from "./statusTone";

/**
 * One dense row in the jobs console. The whole row (except the checkbox and
 * action buttons) is a single expand toggle — click anywhere to drop the
 * `JobDebugPanel` with the full error, decoded spec, and tenant context.
 *
 * A 3px left accent carries the status colour so a wall of rows reads as a
 * status heat-strip down the left edge. `dead` rows get retry/delete; `failed`
 * rows are observe-only (the reaper still owns their lifecycle).
 */
export const JobRow = ({
  row,
  selected,
  onToggleSelect,
  expanded,
  onToggleExpand,
  onRetry,
  onDelete,
  retrying,
  deleting
}: {
  row: QueueRow;
  selected: boolean;
  onToggleSelect: () => void;
  expanded: boolean;
  onToggleExpand: () => void;
  onRetry: () => void;
  onDelete: () => void;
  retrying: boolean;
  deleting: boolean;
}) => {
  const tone = statusTone(row.queue_status);
  const isDead = row.queue_status === "dead";
  const tenant = tenantLabel(row);

  return (
    <div
      className={cn("border-border/50 border-l-2", borderForStatus(row.queue_status))}
      data-testid={`admin-internal-jobs-row-${row.task_id}`}
    >
      <div
        className={cn(
          "grid grid-cols-[auto_auto_minmax(0,1fr)_auto_auto_auto_auto] items-center gap-3 px-3 py-1.5 transition-colors",
          selected ? "bg-muted/40" : "hover:bg-muted/20"
        )}
      >
        <Checkbox
          checked={selected}
          onCheckedChange={onToggleSelect}
          disabled={!isDead}
          aria-label={`Select ${row.task_id}`}
          className={cn(!isDead && "invisible")}
        />

        <button
          type='button'
          onClick={onToggleExpand}
          className='flex items-center gap-2 text-left'
          aria-expanded={expanded}
        >
          {expanded ? (
            <ChevronDown className='size-3.5 shrink-0 text-muted-foreground' />
          ) : (
            <ChevronRight className='size-3.5 shrink-0 text-muted-foreground' />
          )}
          <span className={cn("size-1.5 shrink-0 rounded-full", tone.accent)} aria-hidden />
          <span className='font-mono text-[11px]'>{row.task_type ?? "?"}</span>
          <span className={cn("font-medium text-[10px] uppercase tracking-wide", tone.text)}>
            {row.queue_status}
          </span>
        </button>

        <button
          type='button'
          onClick={onToggleExpand}
          className='flex min-w-0 items-center gap-1.5 text-left'
        >
          <span className='truncate text-xs'>{tenant.workspace}</span>
          {tenant.org ? (
            <>
              <span className='shrink-0 text-muted-foreground/50'>·</span>
              <span className='shrink-0 truncate text-muted-foreground text-xs'>{tenant.org}</span>
            </>
          ) : null}
        </button>

        <span className='max-w-40 truncate font-mono text-[10px] text-muted-foreground'>
          {row.worker_id ?? "—"}
        </span>
        <span className='text-muted-foreground text-xs tabular-nums'>
          {row.claim_count}/{row.max_claims}
        </span>
        <span className='w-14 text-right text-muted-foreground text-xs tabular-nums'>
          {relativeTime(row.updated_at)}
        </span>

        <div className='flex shrink-0 items-center gap-0.5'>
          {isDead ? (
            <>
              <Button
                size='sm'
                variant='ghost'
                disabled={retrying}
                onClick={onRetry}
                className='h-6 w-6 p-0 text-muted-foreground hover:text-foreground'
                aria-label='Re-enqueue'
                title='Re-enqueue'
              >
                {retrying ? (
                  <Loader2 className='size-3.5 animate-spin' />
                ) : (
                  <RotateCcw className='size-3.5' />
                )}
              </Button>
              <Button
                size='sm'
                variant='ghost'
                disabled={deleting}
                onClick={onDelete}
                className='h-6 w-6 p-0 text-muted-foreground hover:bg-destructive/10 hover:text-destructive'
                aria-label='Delete'
                title='Delete'
              >
                {deleting ? (
                  <Loader2 className='size-3.5 animate-spin' />
                ) : (
                  <Trash2 className='size-3.5' />
                )}
              </Button>
            </>
          ) : null}
        </div>
      </div>

      {expanded ? <JobDebugPanel row={row} /> : null}
    </div>
  );
};

function borderForStatus(status: string): string {
  switch (status) {
    case "dead":
      return "border-l-destructive";
    case "failed":
      return "border-l-warning";
    default:
      return "border-l-transparent";
  }
}

function tenantLabel(row: QueueRow): { workspace: string; org: string | null } {
  if (row.workspace_name) {
    return { workspace: row.workspace_name, org: row.org_name };
  }
  if (row.workspace_id) {
    return { workspace: row.workspace_id, org: row.org_name };
  }
  // No run/workspace joined — a system job (e.g. preagg cycle).
  return { workspace: "system", org: null };
}
