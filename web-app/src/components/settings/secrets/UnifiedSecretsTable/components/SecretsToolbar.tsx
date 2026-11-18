import { Search } from "lucide-react";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/shadcn/input-group";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import type { AppRef, SecretsFilter, SecretsScope } from "../types";

const FILTERS: { value: SecretsFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "shared", label: "Shared with apps" },
  { value: "unshared", label: "Not shared with apps" }
];

/** The app filter's value for "don't narrow by app". A `Select` cannot hold "". */
const ANY_APP = "all";

interface SecretsToolbarProps {
  scope: SecretsScope;
  query: string;
  onQuery: (query: string) => void;
  /** Workspace tab: whether any app reads the secret. */
  filter: SecretsFilter;
  onFilter: (filter: SecretsFilter) => void;
  counts: Record<SecretsFilter, number>;
  /** Custom apps tab: the apps that own at least one secret, and how many. */
  apps: AppRef[];
  appCounts: Map<string, number>;
  appId?: string;
  onApp: (appId?: string) => void;
}

/**
 * Search, plus the one way of narrowing that the open tab actually has.
 *
 * Workspace secrets divide by whether an app reads them, which is two chips.
 * App secrets divide by app, which is a list that grows with the workspace — so
 * that one is a select, and it names the count beside each app.
 */
export const SecretsToolbar = ({
  scope,
  query,
  onQuery,
  filter,
  onFilter,
  counts,
  apps,
  appCounts,
  appId,
  onApp
}: SecretsToolbarProps) => {
  // A chip that would select nothing is noise; the one in use always stays, so
  // it can be cleared.
  const offered = FILTERS.filter(
    (f) => f.value === "all" || f.value === filter || counts[f.value] > 0
  );
  return (
    <div className='flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between'>
      <InputGroup className='sm:max-w-xs'>
        <InputGroupAddon>
          <Search />
        </InputGroupAddon>
        <InputGroupInput
          placeholder={scope === "app" ? "Search keys or apps" : "Search secrets or apps"}
          aria-label='Search secrets'
          value={query}
          onChange={(e) => onQuery(e.target.value)}
          data-testid='secrets-search'
        />
      </InputGroup>

      {scope === "workspace" && offered.length > 2 && (
        <ToggleGroup
          type='single'
          size='sm'
          variant='outline'
          value={filter}
          onValueChange={(value) => onFilter((value || "all") as SecretsFilter)}
          aria-label='Filter secrets'
          className='flex-wrap justify-start'
        >
          {offered.map((f) => (
            <ToggleGroupItem
              key={f.value}
              value={f.value}
              className='whitespace-nowrap px-2.5 text-xs'
              data-testid={`secrets-filter-${f.value}`}
            >
              {f.label}
              <span className='ml-1 text-muted-foreground tabular-nums'>{counts[f.value]}</span>
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      )}

      {scope === "app" && apps.length > 1 && (
        <Select
          value={appId ?? ANY_APP}
          onValueChange={(value) => onApp(value === ANY_APP ? undefined : value)}
        >
          <SelectTrigger
            size='sm'
            className='sm:w-56'
            aria-label='Filter by app'
            data-testid='secrets-app-filter'
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ANY_APP}>All apps</SelectItem>
            {apps.map((app) => (
              <SelectItem key={app.id} value={app.id}>
                {app.name}
                <span className='ml-1 text-muted-foreground tabular-nums'>
                  {appCounts.get(app.id) ?? 0}
                </span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
    </div>
  );
};
