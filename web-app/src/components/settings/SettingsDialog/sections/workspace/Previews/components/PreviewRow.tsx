import { Beaker, ChevronDown, ChevronRight, Eye, RefreshCw, Trash2 } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import {
  useCreatePreview,
  useDeletePreview,
  useRefreshPreview
} from "@/hooks/api/workspaces/usePreviews";
import { dateAgo, parseUtcTimestamp } from "@/libs/utils/date";
import type { WorkspacePreview } from "@/types/workspace";
import DeletePreviewDialog from "./DeletePreviewDialog";
import PreviewChecksPanel from "./PreviewChecksPanel";
import PreviewRowNotice, { rowNoticeFor } from "./PreviewRowNotice";
import PreviewRunsPanel from "./PreviewRunsPanel";
import PreviewStatusBadge from "./PreviewStatusBadge";

interface Props {
  workspaceId: string;
  preview: WorkspacePreview;
  onOpen: (preview: WorkspacePreview) => void;
}

function LastCompiled({ value }: { value: string | null }) {
  if (!value) return <span className='text-muted-foreground'>Never</span>;
  const date = parseUtcTimestamp(value);
  if (!date) return <span className='text-muted-foreground'>—</span>;
  return <span title={date.toLocaleString()}>{dateAgo(date)}</span>;
}

export default function PreviewRow({ workspaceId, preview, onOpen }: Props) {
  const [errorOpen, setErrorOpen] = useState(false);
  const [runsOpen, setRunsOpen] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const refresh = useRefreshPreview(workspaceId);
  const create = useCreatePreview(workspaceId);
  const remove = useDeletePreview(workspaceId);

  const { branch, status } = preview;
  const failed = status === "failed";
  // Also covers "no revision and no compile queued" (a compile that died
  // before starting, or a revision retention removed): the same Refresh.
  const stale = status === "stale";
  const testId = `preview-row-${branch}`;
  const notice = rowNoticeFor(refresh.error, create.error);
  const recompile = () => {
    create.reset();
    refresh.mutate(branch);
  };
  const createInstead = () => create.mutate(branch, { onSuccess: () => refresh.reset() });

  return (
    <>
      <TableRow data-testid={testId} data-status={status}>
        <TableCell data-label='Branch' className='font-mono'>
          {branch}
          {preview.sha && (
            <span className='ml-2 text-muted-foreground text-xs' title={preview.sha}>
              @ {preview.sha.slice(0, 7)}
            </span>
          )}
        </TableCell>
        <TableCell data-label='Status'>
          <div className='flex items-center gap-2'>
            <PreviewStatusBadge status={status} />
            {stale && (
              // The branch moved on since this compile (or there is no revision
              // and nothing queued). Say so where the fix is, instead of leaving
              // a silent "Stale".
              <Button
                variant='link'
                size='sm'
                className='h-6 px-0 text-xs'
                onClick={recompile}
                disabled={refresh.isPending}
                data-testid={`${testId}-stale-refresh`}
              >
                Branch moved — refresh to recompile
              </Button>
            )}
            {failed && preview.error && (
              <Button
                variant='ghost'
                size='sm'
                className='h-6 px-1.5 text-xs'
                aria-expanded={errorOpen}
                onClick={() => setErrorOpen((v) => !v)}
                data-testid={`${testId}-toggle-error`}
              >
                {errorOpen ? <ChevronDown /> : <ChevronRight />}
                {errorOpen ? "Hide error" : "Show error"}
              </Button>
            )}
          </div>
        </TableCell>
        <TableCell data-label='Checks'>
          <PreviewChecksPanel
            workspaceId={workspaceId}
            branch={branch}
            summary={preview.checks}
            testId={testId}
          />
        </TableCell>
        <TableCell data-label='Author'>
          {preview.created_by?.name ?? <span className='text-muted-foreground'>—</span>}
        </TableCell>
        <TableCell data-label='Last compiled'>
          <LastCompiled value={preview.compiled_at} />
        </TableCell>
        <TableCell>
          <div className='flex items-center justify-end gap-1'>
            {status === "ready" && preview.revision_id && (
              <Button
                size='sm'
                variant='outline'
                onClick={() => onOpen(preview)}
                data-testid={`${testId}-open`}
              >
                <Eye />
                Open
              </Button>
            )}
            <Button
              size='sm'
              variant='ghost'
              onClick={recompile}
              disabled={refresh.isPending || status === "compiling"}
              tooltip='Recompile this branch'
              aria-label={`Refresh preview ${branch}`}
              data-testid={`${testId}-refresh`}
            >
              <RefreshCw />
            </Button>
            <Button
              size='sm'
              variant='ghost'
              onClick={() => setRunsOpen((v) => !v)}
              tooltip='Dry-run a procedure'
              aria-expanded={runsOpen}
              aria-label={`Runs for preview ${branch}`}
              data-testid={`${testId}-runs-toggle`}
            >
              <Beaker />
            </Button>
            <Button
              size='sm'
              variant='ghost'
              onClick={() => setConfirmDelete(true)}
              disabled={remove.isPending}
              tooltip='Delete this preview'
              aria-label={`Delete preview ${branch}`}
              data-testid={`${testId}-delete`}
            >
              <Trash2 className='text-destructive' />
            </Button>
          </div>
        </TableCell>
      </TableRow>
      {failed && preview.error && errorOpen && (
        <TableRow data-testid={`${testId}-error`}>
          <TableCell colSpan={6}>
            <pre className='max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-md bg-muted p-3 font-mono text-destructive text-xs'>
              {preview.error}
            </pre>
          </TableCell>
        </TableRow>
      )}
      {runsOpen && (
        <TableRow data-testid={`${testId}-runs`}>
          <TableCell colSpan={6}>
            <PreviewRunsPanel workspaceId={workspaceId} branch={branch} />
          </TableCell>
        </TableRow>
      )}
      {notice && (
        <PreviewRowNotice
          testId={testId}
          notice={notice}
          onCreate={createInstead}
          isCreating={create.isPending}
        />
      )}
      <DeletePreviewDialog
        branch={branch}
        open={confirmDelete}
        onOpenChange={setConfirmDelete}
        onConfirm={() => remove.mutate(branch, { onSettled: () => setConfirmDelete(false) })}
        isPending={remove.isPending}
      />
    </>
  );
}
