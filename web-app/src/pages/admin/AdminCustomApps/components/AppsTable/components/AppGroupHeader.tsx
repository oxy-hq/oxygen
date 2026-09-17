import { ChevronRight } from "lucide-react";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";

interface AppGroupHeaderProps {
  label: string;
  count: number;
  collapsed: boolean;
  onToggleCollapse: () => void;
  /** Tri-state select for every row in the group. */
  checked: boolean | "indeterminate";
  onToggleGroup: () => void;
  /** Columns the label cell spans (total columns minus the checkbox cell). */
  labelColSpan: number;
}

/**
 * Sticky-under-header group divider: a group-select checkbox, a collapse
 * chevron, the group label, and its row count. Selecting here toggles every
 * row in the group; collapsing hides them without losing selection.
 */
export const AppGroupHeader = ({
  label,
  count,
  collapsed,
  onToggleCollapse,
  checked,
  onToggleGroup,
  labelColSpan
}: AppGroupHeaderProps) => (
  <TableRow className='bg-muted/20 hover:bg-muted/20'>
    <TableCell className='w-10'>
      <Checkbox
        checked={checked}
        onCheckedChange={onToggleGroup}
        aria-label={`Select all in ${label}`}
        className='opacity-60 transition-opacity hover:opacity-100 data-[state=checked]:opacity-100'
      />
    </TableCell>
    <TableCell colSpan={labelColSpan} className='py-1.5'>
      <button
        type='button'
        onClick={onToggleCollapse}
        className='flex items-center gap-1.5 text-left text-muted-foreground outline-none hover:text-foreground focus-visible:underline'
        aria-expanded={!collapsed}
      >
        <ChevronRight className={cn("size-3 transition-transform", !collapsed && "rotate-90")} />
        <span className='font-medium text-foreground text-xs'>{label}</span>
        <span className='text-muted-foreground/60 text-xs tabular-nums'>{count}</span>
      </button>
    </TableCell>
  </TableRow>
);
