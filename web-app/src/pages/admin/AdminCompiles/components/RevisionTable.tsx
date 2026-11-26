import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import type { useRowSelection } from "@/hooks/useRowSelection";
import type { CompileRow } from "@/services/api/compiles";
import { RevisionRow } from "./RevisionRow";

type Selection = ReturnType<typeof useRowSelection>;

const HEAD_CLS =
  "h-8 bg-muted/40 font-medium text-[10px] text-muted-foreground uppercase tracking-wider";

/**
 * Flat "All revisions" data-grid — sticky header, hairline borders,
 * select-all checkbox wired to the parent's selection hook. The header
 * checkbox reflects all/some/none via `indeterminate`.
 */
export const RevisionTable = ({
  rows,
  selection
}: {
  rows: CompileRow[];
  selection: Selection;
}) => (
  <div className='overflow-hidden border border-border/60 bg-card'>
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
                aria-label='Select all revisions'
              />
            </TableHead>
            <TableHead className={HEAD_CLS}>Status</TableHead>
            <TableHead className={HEAD_CLS}>Revision</TableHead>
            <TableHead className={HEAD_CLS}>Git</TableHead>
            <TableHead className={HEAD_CLS}>Files</TableHead>
            <TableHead className={HEAD_CLS}>Duration</TableHead>
            <TableHead className={HEAD_CLS}>Started</TableHead>
            <TableHead className={HEAD_CLS}>Compiler</TableHead>
            <TableHead className={`${HEAD_CLS} pr-3 text-right`}>Actions</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((row) => (
            <RevisionRow
              key={row.revision_id}
              row={row}
              selected={selection.isSelected(row.revision_id)}
              onToggle={selection.toggle}
            />
          ))}
        </TableBody>
      </Table>
    </div>
  </div>
);
