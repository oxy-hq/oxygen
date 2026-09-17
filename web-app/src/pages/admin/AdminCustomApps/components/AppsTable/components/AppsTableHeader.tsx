import { ArrowDown, ArrowUp } from "lucide-react";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";
import type { SortDir, SortKey } from "../useAppsTable";

interface AppsTableHeaderProps {
  showOrg: boolean;
  sortKey: SortKey;
  sortDir: SortDir;
  onSort: (key: SortKey) => void;
  allSelected: boolean;
  someSelected: boolean;
  onToggleAll: () => void;
}

/**
 * The request window the Requests column reports. Fixed by the backend — it is
 * the heartbeat window (`oxy_observability::heartbeat::WINDOW_MINUTES`), which the
 * fleet endpoint also uses for its traffic count.
 */
const REQUEST_WINDOW_LABEL = "6h";

/**
 * Sticky column header. Only the active sort column carries an arrow; the
 * always-faint up/down hint on every other column is gone — sortable columns
 * already read as clickable, and five hint glyphs were more of the same noise.
 */
export const AppsTableHeader = ({
  showOrg,
  sortKey,
  sortDir,
  onSort,
  allSelected,
  someSelected,
  onToggleAll
}: AppsTableHeaderProps) => (
  <TableHeader className='sticky top-0 z-10 bg-background'>
    <TableRow className='hover:bg-transparent'>
      <TableHead className='w-9 pr-0'>
        <Checkbox
          checked={allSelected ? true : someSelected ? "indeterminate" : false}
          onCheckedChange={onToggleAll}
          aria-label='Select all apps'
          className={cn(
            "transition-opacity",
            allSelected || someSelected ? "opacity-100" : "opacity-40 hover:opacity-100"
          )}
        />
      </TableHead>
      <SortHead col='name' label='App' active={sortKey} dir={sortDir} onSort={onSort} />
      {showOrg && <SortHead col='org' label='Org' active={sortKey} dir={sortDir} onSort={onSort} />}
      <SortHead col='status' label='Status' active={sortKey} dir={sortDir} onSort={onSort} />
      <TableHead className='text-right font-medium text-muted-foreground'>
        Requests ({REQUEST_WINDOW_LABEL})
      </TableHead>
      <SortHead
        col='active'
        label='Last active'
        active={sortKey}
        dir={sortDir}
        onSort={onSort}
        align='right'
      />
      <TableHead className='w-10'>
        <span className='sr-only'>Actions</span>
      </TableHead>
    </TableRow>
  </TableHeader>
);

const SortHead = ({
  col,
  label,
  active,
  dir,
  onSort,
  align = "left"
}: {
  col: SortKey;
  label: string;
  active: SortKey;
  dir: SortDir;
  onSort: (key: SortKey) => void;
  align?: "left" | "right";
}) => {
  const isActive = active === col;
  return (
    <TableHead
      className='p-0'
      aria-sort={isActive ? (dir === "asc" ? "ascending" : "descending") : undefined}
    >
      <button
        type='button'
        onClick={() => onSort(col)}
        className={cn(
          "flex h-full w-full items-center gap-1 px-2 py-2 font-medium outline-none focus-visible:underline",
          align === "right" ? "justify-end" : "text-left",
          isActive ? "text-foreground" : "text-muted-foreground hover:text-foreground"
        )}
      >
        {label}
        {isActive &&
          (dir === "asc" ? <ArrowUp className='size-3' /> : <ArrowDown className='size-3' />)}
      </button>
    </TableHead>
  );
};
