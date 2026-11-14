import { Edit, MoreHorizontal, Share2, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import type { UnifiedRow } from "../types";

/**
 * The row's verbs, one click from the table: share (a workspace secret only —
 * an app's own secret belongs to that app), change the value, delete. The click
 * never reaches the row, which opens the detail view.
 */
export const RowActions = ({
  row,
  onShare,
  onEdit,
  onDelete
}: {
  row: UnifiedRow;
  onShare: (row: UnifiedRow) => void;
  onEdit: (row: UnifiedRow) => void;
  onDelete: (row: UnifiedRow) => void;
}) => (
  <DropdownMenu>
    <DropdownMenuTrigger asChild>
      <Button
        variant='ghost'
        size='sm'
        className='size-7 p-0'
        aria-label={`Actions for ${row.name}`}
        data-testid='secret-row-actions'
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.stopPropagation()}
      >
        <MoreHorizontal className='size-4' />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align='end' onClick={(e) => e.stopPropagation()}>
      {!row.app && (
        <DropdownMenuItem onSelect={() => onShare(row)} data-testid='secret-row-share'>
          <Share2 />
          Share with apps…
        </DropdownMenuItem>
      )}
      <DropdownMenuItem onSelect={() => onEdit(row)}>
        <Edit />
        Change value
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem variant='destructive' onSelect={() => onDelete(row)}>
        <Trash2 />
        Delete
      </DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>
);
