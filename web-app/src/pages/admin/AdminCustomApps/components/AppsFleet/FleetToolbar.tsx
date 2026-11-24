import { ArrowUpCircle, Search } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { AppStatus } from "../../appStatus";
import type { FleetFilter } from "../../fleetFilters";
import { statusTone } from "../AppSwitcher";

/**
 * The fleet table's controls: filter by text, narrow to one verdict, see how much of the
 * list is showing, and promote the latest build across what is in view.
 *
 * **The chips' colour comes from `statusTone`, never from a down/not-down guess.**
 * `discriminatingFilters` returns a chip for any status that forms a proper subset, so an
 * "Operational 2" chip was once painted in the warning tone — and on this surface colour
 * means something is wrong.
 *
 * **The count says "n of m" only when it is narrowing something**, and both numbers are
 * the scope's, not the deployment's: under one org, "3 of 40" about three apps would be a
 * claim about a fleet this view cannot see.
 */
export const FleetToolbar = ({
  query,
  onQueryChange,
  filters,
  status,
  onStatusChange,
  counts,
  promote
}: {
  query: string;
  onQueryChange: (next: string) => void;
  filters: readonly FleetFilter[];
  status: AppStatus | null;
  onStatusChange: (next: AppStatus | null) => void;
  /** Rows on screen, and rows in scope before the filter. */
  counts: { shown: number; total: number };
  promote: { count: number; disabled: boolean; onOpen: () => void };
}) => (
  <div className='flex items-center gap-2 px-4 py-2'>
    <div className='relative min-w-0 flex-1 md:max-w-xs'>
      <Search className='absolute top-1/2 left-2 size-3 -translate-y-1/2 text-muted-foreground' />
      <Input
        value={query}
        onChange={(e) => onQueryChange(e.target.value)}
        placeholder='Filter by name, slug or org…'
        className='h-7 pl-7 text-xs'
        data-testid='admin-apps-fleet-search'
      />
    </div>
    {filters.map((f) => (
      <button
        key={f.status}
        type='button'
        onClick={() => onStatusChange(status === f.status ? null : f.status)}
        data-testid={`admin-apps-fleet-filter-${f.status}`}
        className={cn(
          "h-7 shrink-0 rounded-md border px-2 text-xs transition-colors",
          status === f.status
            ? cn(ADMIN_TONE[statusTone(f.status)].bg, "border-transparent")
            : "border-border/60 text-muted-foreground hover:text-foreground"
        )}
      >
        {f.label} <span className='tabular-nums'>{f.count}</span>
      </button>
    ))}
    <span className='ml-auto shrink-0 text-muted-foreground text-xs tabular-nums'>
      {counts.shown === counts.total
        ? `${counts.total} app${counts.total === 1 ? "" : "s"}`
        : `${counts.shown} of ${counts.total}`}
    </span>
    <Button
      variant='outline'
      size='sm'
      className='h-7 shrink-0 gap-1.5 text-xs'
      disabled={promote.disabled}
      onClick={promote.onOpen}
      data-testid='admin-apps-fleet-promote-latest'
    >
      <ArrowUpCircle className='size-3.5' />
      Promote latest
      <span className='text-muted-foreground tabular-nums'>{promote.count}</span>
    </Button>
  </div>
);
