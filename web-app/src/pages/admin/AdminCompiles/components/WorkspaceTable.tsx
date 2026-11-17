import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import type { useRowSelection } from "@/hooks/useRowSelection";
import type { WorkspaceCompileRow } from "@/services/api/compiles";
import { WorkspaceRow } from "./WorkspaceRow";

type Selection = ReturnType<typeof useRowSelection>;

const HEAD_CLS =
  "h-8 bg-muted/40 font-medium text-[10px] text-muted-foreground uppercase tracking-wider";

/**
 * Aggregated "By workspace" grid — the default view. Sticky header,
 * workspace select-all, and rows that expand into per-workspace revision
 * history. Workspace selection feeds "Recompile selected"; expanded
 * revision selection feeds "Promote selected".
 */
export const WorkspaceTable = ({
  rows,
  paused,
  selection,
  revisionSelection
}: {
  rows: WorkspaceCompileRow[];
  paused: boolean;
  selection: Selection;
  revisionSelection: Selection;
}) => (
  <div className='overflow-hidden rounded-lg border border-border/60 bg-card'>
    <div className='max-h-[calc(100vh-18rem)] overflow-auto'>
      <Table className='text-xs'>
        <TableHeader className='sticky top-0 z-10'>
          <TableRow className='border-border/60 hover:bg-transparent'>
            <TableHead className={`${HEAD_CLS} w-9 pl-3`}>
              <Checkbox
                checked={
                  selection.allSelected ? true : selection.someSelected ? "indeterminate" : false
                }
                onCheckedChange={selection.toggleAll}
                aria-label='Select all workspaces'
              />
            </TableHead>
            <TableHead className={`${HEAD_CLS} w-7 pr-0`} />
            <TableHead className={`${HEAD_CLS} w-[26%]`}>Workspace</TableHead>
            <TableHead className={HEAD_CLS}>State</TableHead>
            <TableHead className={`${HEAD_CLS} w-[12%]`}>Git</TableHead>
            <TableHead className={HEAD_CLS}>Revisions</TableHead>
            <TableHead className={HEAD_CLS}>Last ready</TableHead>
            <TableHead className={`${HEAD_CLS} pr-3`}>Latest run</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((row) => (
            <WorkspaceRow
              key={row.workspace_id}
              row={row}
              paused={paused}
              selected={selection.isSelected(row.workspace_id)}
              onToggleWorkspace={selection.toggle}
              isRevisionSelected={revisionSelection.isSelected}
              onToggleRevision={revisionSelection.toggle}
            />
          ))}
        </TableBody>
      </Table>
    </div>
  </div>
);
