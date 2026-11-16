import { Edit, MoreHorizontal, Share2, Trash2, Users } from "lucide-react";
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
 * The row's verbs. The click never reaches the row, which opens the detail
 * view.
 *
 * A workspace secret can be handed to more apps in place. An app's own secret
 * cannot — it is stored under that app and nothing else can read it — so its
 * menu offers the one move that gets there: store the key as a workspace
 * secret instead, which can then be shared with as many apps as you like.
 */
export const RowActions = ({
  row,
  onShare,
  onEdit,
  onDelete,
  onLift
}: {
  row: UnifiedRow;
  onShare: (row: UnifiedRow) => void;
  onEdit: (row: UnifiedRow) => void;
  onDelete: (row: UnifiedRow) => void;
  onLift: (row: UnifiedRow) => void;
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
      {row.app ? (
        <DropdownMenuItem onSelect={() => onLift(row)} data-testid='secret-row-lift'>
          <Users />
          Share with more apps…
        </DropdownMenuItem>
      ) : (
        <DropdownMenuItem onSelect={() => onShare(row)} data-testid='secret-row-share'>
          <Share2 />
          Choose apps…
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
