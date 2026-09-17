import { ChevronRight } from "lucide-react";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { cn } from "@/libs/shadcn/utils";

/**
 * Group divider for the gallery (non-table) view: a group-select checkbox, a
 * collapse chevron, the label, and the count. Mirrors the list's group row.
 */
export const GroupSectionHeader = ({
  label,
  count,
  collapsed,
  onToggleCollapse,
  checked,
  onToggleGroup
}: {
  label: string;
  count: number;
  collapsed: boolean;
  onToggleCollapse: () => void;
  checked: boolean | "indeterminate";
  onToggleGroup: () => void;
}) => (
  <div className='flex items-center gap-2'>
    <Checkbox
      checked={checked}
      onCheckedChange={onToggleGroup}
      aria-label={`Select all in ${label}`}
      className='opacity-60 transition-opacity hover:opacity-100 data-[state=checked]:opacity-100'
    />
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
  </div>
);
