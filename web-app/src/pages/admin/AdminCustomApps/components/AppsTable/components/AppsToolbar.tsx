import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { Input } from "@/components/ui/shadcn/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import type { AppsTableState, GroupBy, StatusCounts, ViewMode } from "../useAppsTable";
import { StatusFilterChips } from "./StatusFilterChips";

interface AppsToolbarProps {
  state: AppsTableState;
  setState: (patch: Partial<AppsTableState>) => void;
  onCreate: () => void;
  counts: StatusCounts;
  /** Apps the other filters let through — the `All` chip's number. */
  scopedTotal: number;
  orgs: string[];
  /** Why the status column may be blank for published apps, if it is. */
  healthNote: string | null;
}

/**
 * Two quiet rows: find (search, org, view, new app), then triage (status chips).
 *
 * This replaces a fleet strip, three selects, a layout toggle, a bare count and an
 * icon-only create button. The fleet strip's numbers live in the chips now, where
 * they can be clicked. Layout and grouping are rarely changed, so they sit behind
 * one View menu. Nothing that filters lives there: a filter hidden in a menu is a
 * filter nobody remembers setting.
 */
export const AppsToolbar = ({
  state,
  setState,
  onCreate,
  counts,
  scopedTotal,
  orgs,
  healthNote
}: AppsToolbarProps) => (
  <div className='shrink-0 space-y-2 border-b px-4 py-3' data-testid='admin-apps-toolbar'>
    <div className='flex items-center gap-2'>
      <Input
        value={state.q}
        onChange={(e) => setState({ q: e.target.value })}
        placeholder='Search apps, orgs, workspaces'
        aria-label='Search apps'
        className='h-8 max-w-md flex-1 text-xs'
        data-testid='admin-apps-search'
      />

      <Select value={state.org} onValueChange={(org) => setState({ org })}>
        <SelectTrigger
          className='h-8 w-auto min-w-28 gap-1.5 px-2.5 text-xs'
          aria-label='Filter by org'
          data-testid='admin-apps-org-filter'
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent align='end'>
          <SelectItem value='all'>All orgs</SelectItem>
          {orgs.map((org) => (
            <SelectItem key={org} value={org}>
              {org}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>

      <ViewMenu state={state} setState={setState} />

      <Button size='sm' className='ml-auto h-8' onClick={onCreate} data-testid='admin-apps-new'>
        New app
      </Button>
    </div>

    <div className='flex flex-wrap items-center gap-2'>
      <StatusFilterChips
        counts={counts}
        total={scopedTotal}
        active={state.status}
        onChange={(status) => setState({ status })}
      />
      {healthNote && (
        <span
          className='ml-auto text-muted-foreground text-xs'
          data-testid='admin-apps-health-note'
        >
          {healthNote}
        </span>
      )}
    </div>
  </div>
);

const ViewMenu = ({
  state,
  setState
}: {
  state: AppsTableState;
  setState: (patch: Partial<AppsTableState>) => void;
}) => (
  <DropdownMenu>
    <DropdownMenuTrigger asChild>
      <Button variant='outline' size='sm' className='h-8' data-testid='admin-apps-view-menu'>
        View
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align='end' className='w-44'>
      <DropdownMenuLabel className='text-muted-foreground text-xs'>Layout</DropdownMenuLabel>
      <DropdownMenuRadioGroup
        value={state.view}
        onValueChange={(v) => setState({ view: v as ViewMode })}
      >
        <DropdownMenuRadioItem value='list'>List</DropdownMenuRadioItem>
        <DropdownMenuRadioItem value='gallery'>Cards</DropdownMenuRadioItem>
      </DropdownMenuRadioGroup>

      <DropdownMenuSeparator />
      <DropdownMenuLabel className='text-muted-foreground text-xs'>Group by</DropdownMenuLabel>
      <DropdownMenuRadioGroup
        value={state.group}
        onValueChange={(v) => setState({ group: v as GroupBy })}
      >
        <DropdownMenuRadioItem value='none'>Nothing</DropdownMenuRadioItem>
        <DropdownMenuRadioItem value='status'>Status</DropdownMenuRadioItem>
        <DropdownMenuRadioItem value='org'>Org</DropdownMenuRadioItem>
      </DropdownMenuRadioGroup>
    </DropdownMenuContent>
  </DropdownMenu>
);
