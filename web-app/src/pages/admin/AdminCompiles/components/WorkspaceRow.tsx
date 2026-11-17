import { ChevronRight } from "lucide-react";
import { Fragment, useState } from "react";
import { Link } from "react-router-dom";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { useWorkspaceRevisions } from "@/hooks/api/compiles";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { CopyableId } from "@/pages/admin/components/CopyableId";
import type { WorkspaceCompileRow } from "@/services/api/compiles";
import { compileState, STATE_LABEL, STATE_TONE } from "../compileState";
import { formatRelative } from "../utils";
import { RevisionRow } from "./RevisionRow";

type RevisionToggle = (id: string, shiftKey: boolean) => void;

const SUB_HEAD =
  "h-7 bg-muted/30 font-medium text-[10px] text-muted-foreground uppercase tracking-wider";

/**
 * Dense one-line summary of a workspace's compile state. The chevron
 * expands the row to lazily fetch and render that workspace's revision
 * history (`GET /admin/compiles?workspace_id=`). The workspace checkbox
 * feeds the "Recompile selected" batch; expanded revisions feed
 * "Promote selected" via the shared revision selection.
 */
export const WorkspaceRow = ({
  row,
  paused,
  selected,
  onToggleWorkspace,
  isRevisionSelected,
  onToggleRevision
}: {
  row: WorkspaceCompileRow;
  paused: boolean;
  selected: boolean;
  onToggleWorkspace: (id: string, shiftKey: boolean) => void;
  isRevisionSelected: (id: string) => boolean;
  onToggleRevision: RevisionToggle;
}) => {
  const [expanded, setExpanded] = useState(false);
  const revisions = useWorkspaceRevisions(row.workspace_id, { enabled: expanded, paused });
  const state = compileState(row);
  const tone = STATE_TONE[state];

  return (
    <Fragment>
      <TableRow
        data-state={selected ? "selected" : undefined}
        data-testid={`admin-compiles-workspace-row-${row.workspace_id}`}
      >
        <TableCell className='pl-3'>
          <Checkbox
            checked={selected}
            onClick={(e) => onToggleWorkspace(row.workspace_id, e.shiftKey)}
            aria-label='Select workspace'
          />
        </TableCell>
        <TableCell className='w-7 pr-0'>
          <button
            type='button'
            onClick={() => setExpanded((v) => !v)}
            aria-label={expanded ? "Collapse revisions" : "Expand revisions"}
            aria-expanded={expanded}
            className='inline-flex size-5 items-center justify-center rounded-sm text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring'
          >
            <ChevronRight
              className={cn("size-3.5 transition-transform", expanded && "rotate-90")}
            />
          </button>
        </TableCell>
        <TableCell className='max-w-0'>
          <div className='flex flex-col leading-tight'>
            <span className='truncate font-medium'>{row.workspace_name ?? "—"}</span>
            {/* The org first: four workspaces called `pokehouse-oxy` were told apart
                only by an id prefix. */}
            <span className='flex min-w-0 items-center gap-1.5'>
              {row.org_name ? (
                <span className='truncate text-muted-foreground'>{row.org_name}</span>
              ) : null}
              <CopyableId value={row.workspace_id} head={8} />
            </span>
          </div>
        </TableCell>
        <TableCell data-testid={`admin-compiles-workspace-state-${row.workspace_id}`}>
          {/* The state against the last good revision, not the current revision's own
              status: "ready" is true of a workspace serving an old revision while every
              compile since has failed. Colour only when it is wrong. */}
          <div className='flex flex-col leading-tight'>
            <span className={cn("flex items-center gap-1.5 font-medium", ADMIN_TONE[tone].text)}>
              {tone !== "muted" ? (
                <span className={cn("size-1.5 rounded-full", ADMIN_TONE[tone].dot)} aria-hidden />
              ) : null}
              {STATE_LABEL[state]}
            </span>
            {state === "failing" ? (
              <span className='text-[10px] text-muted-foreground'>
                {row.last_ready_at
                  ? `serving a revision from ${formatRelative(row.last_ready_at)}`
                  : "no good revision yet"}
                {" · "}
                <Link
                  to={`${ROUTES.ADMIN.INTERNAL_JOBS}?q=${encodeURIComponent(row.workspace_name ?? row.workspace_id)}`}
                  className='underline-offset-2 hover:underline'
                  data-testid={`admin-compiles-workspace-jobs-${row.workspace_id}`}
                >
                  failed jobs
                </Link>
              </span>
            ) : state === "behind" ? (
              <span className='text-[10px] text-muted-foreground'>
                newer good revision not promoted
              </span>
            ) : null}
          </div>
        </TableCell>
        <TableCell>
          <CopyableId value={row.current_git_sha} head={12} />
        </TableCell>
        <TableCell className='tabular-nums'>
          <span className='font-medium'>{row.revision_count}</span>
          <span className='text-muted-foreground'> total</span>
          {/* Historic failures are context, not an alarm: the State column says whether
              the workspace is failing *now*. */}
          {row.failed_count > 0 ? (
            <span className='ml-1 text-muted-foreground'>· {row.failed_count} failed</span>
          ) : null}
        </TableCell>
        <TableCell className='text-muted-foreground tabular-nums'>
          {formatRelative(row.last_ready_at)}
        </TableCell>
        <TableCell className='pr-3 text-muted-foreground tabular-nums'>
          {formatRelative(row.latest_started_at)}
        </TableCell>
      </TableRow>

      {expanded ? (
        <TableRow className='hover:bg-transparent'>
          <TableCell colSpan={8} className='bg-muted/10 p-0'>
            {/* Its own gate: each expanded workspace fetches its own history, so a
                failure here is about this row, not the page. */}
            <AdminAsync
              query={revisions}
              noun='revisions'
              className='m-2'
              skeleton={<Skeleton className='h-16' />}
              isEmpty={(d) => d.rows.length === 0}
              empty={
                <p className='text-muted-foreground text-xs'>No revisions for this workspace.</p>
              }
            >
              {(d) => (
                <div className='border-border/40 border-t'>
                  <Table className='text-xs'>
                    <TableHeader>
                      <TableRow className='hover:bg-transparent'>
                        <TableHead className={`${SUB_HEAD} w-9 pl-3`} />
                        <TableHead className={SUB_HEAD}>Status</TableHead>
                        <TableHead className={SUB_HEAD}>Revision</TableHead>
                        <TableHead className={SUB_HEAD}>Git</TableHead>
                        <TableHead className={SUB_HEAD}>Files</TableHead>
                        <TableHead className={SUB_HEAD}>Duration</TableHead>
                        <TableHead className={SUB_HEAD}>Started</TableHead>
                        <TableHead className={SUB_HEAD}>Compiler</TableHead>
                        <TableHead className={`${SUB_HEAD} pr-3 text-right`}>Actions</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {d.rows.map((rev) => (
                        <RevisionRow
                          key={rev.revision_id}
                          row={rev}
                          nested
                          selected={isRevisionSelected(rev.revision_id)}
                          onToggle={onToggleRevision}
                        />
                      ))}
                    </TableBody>
                  </Table>
                </div>
              )}
            </AdminAsync>
          </TableCell>
        </TableRow>
      ) : null}
    </Fragment>
  );
};
