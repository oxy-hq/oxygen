import { Loader2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { usePromoteCompile } from "@/hooks/api/compiles";
import { cn } from "@/libs/shadcn/utils";
import { CopyableId } from "@/pages/admin/components/CopyableId";
import type { CompileRow } from "@/services/api/compiles";
import { formatMs, formatRelative, toneBadgeClass } from "../utils";
import { CompileDetailSheet } from "./CompileDetailSheet";
import { StatusBadge } from "./StatusBadge";

/**
 * One revision row in either the flat "All revisions" table or a
 * workspace's expanded history. `nested` tones the row down so expanded
 * children read as a sub-table. Carries its own single-row promote lever
 * alongside the batch selection checkbox.
 */
export const RevisionRow = ({
  row,
  selected,
  onToggle,
  nested = false
}: {
  row: CompileRow;
  selected: boolean;
  onToggle: (id: string, shiftKey: boolean) => void;
  nested?: boolean;
}) => {
  const promote = usePromoteCompile();
  const [detailOpen, setDetailOpen] = useState(false);
  const canPromote = row.status === "ready" && row.kind === "main" && !row.is_current_for_workspace;

  const onPromote = () => {
    if (
      !window.confirm(
        `Repoint this workspace to revision ${row.revision_id.slice(0, 8)}? This is a manual ` +
          "rollback — the runtime will immediately serve this revision's definitions."
      )
    )
      return;
    promote.mutate(row.revision_id, {
      onSuccess: () => toast.success("Workspace repointed to this revision."),
      onError: (e: unknown) => toast.error(e instanceof Error ? e.message : "Promote failed")
    });
  };

  return (
    <TableRow
      data-state={selected ? "selected" : undefined}
      className={nested ? "bg-muted/10" : ""}
      data-testid={`admin-compiles-revision-row-${row.revision_id}`}
    >
      <TableCell className='pl-3'>
        <Checkbox
          checked={selected}
          onClick={(e) => onToggle(row.revision_id, e.shiftKey)}
          aria-label='Select revision'
        />
      </TableCell>
      <TableCell>
        <div className='flex items-center gap-1.5'>
          <StatusBadge status={row.status} />
          {row.is_current_for_workspace ? (
            <Badge className={cn(toneBadgeClass("ok"), "px-1.5 py-0 text-[10px]")}>current</Badge>
          ) : null}
          {row.kind === "draft" ? (
            <Badge variant='outline' className='px-1.5 py-0 text-[10px]'>
              draft
            </Badge>
          ) : null}
        </div>
      </TableCell>
      <TableCell>
        <CopyableId value={row.revision_id} head={8} />
      </TableCell>
      <TableCell>
        <div className='flex flex-col leading-tight'>
          <CopyableId value={row.git_sha} head={12} />
          {row.branch ? (
            <span className='px-1 text-[10px] text-muted-foreground'>{row.branch}</span>
          ) : null}
        </div>
      </TableCell>
      <TableCell className='tabular-nums'>
        <button
          type='button'
          onClick={() => setDetailOpen(true)}
          className='rounded text-left hover:underline'
          title='View which entities compiled and which failed'
        >
          <span className='font-medium'>{row.file_count_compiled}</span>
          <span className='text-muted-foreground'> / {row.file_count_seen}</span>
          {row.file_count_failed > 0 ? (
            <span className='ml-1 text-destructive'>({row.file_count_failed} failed)</span>
          ) : null}
        </button>
        <CompileDetailSheet
          revisionId={row.revision_id}
          open={detailOpen}
          onOpenChange={setDetailOpen}
        />
      </TableCell>
      <TableCell className='tabular-nums'>
        {row.duration_ms !== null ? formatMs(row.duration_ms) : "—"}
      </TableCell>
      <TableCell className='text-muted-foreground tabular-nums'>
        {formatRelative(row.started_at)}
      </TableCell>
      <TableCell className='font-mono text-[10px] text-muted-foreground'>
        {row.compiler_version}
      </TableCell>
      <TableCell className='pr-3 text-right'>
        {canPromote ? (
          <Button
            size='sm'
            variant='ghost'
            onClick={onPromote}
            disabled={promote.isPending}
            className='h-6 gap-1 px-2 text-[11px]'
          >
            {promote.isPending ? <Loader2 className='size-3 animate-spin' /> : null}
            Promote
          </Button>
        ) : null}
      </TableCell>
    </TableRow>
  );
};
