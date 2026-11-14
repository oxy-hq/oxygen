import { Search } from "lucide-react";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/shadcn/input-group";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import type { SecretsFilter } from "../types";

const FILTERS: { value: SecretsFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "shared", label: "Shared" },
  { value: "workspace", label: "Workspace only" },
  { value: "app", label: "App-only" }
];

/**
 * Search and filter for the secrets table. A filter with nothing in it is not
 * offered (a chip that selects nothing is noise), but the one in use always is,
 * so it can be cleared.
 */
export const SecretsToolbar = ({
  query,
  onQuery,
  filter,
  onFilter,
  counts
}: {
  query: string;
  onQuery: (query: string) => void;
  filter: SecretsFilter;
  onFilter: (filter: SecretsFilter) => void;
  counts: Record<SecretsFilter, number>;
}) => {
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
          placeholder='Search secrets or apps'
          aria-label='Search secrets'
          value={query}
          onChange={(e) => onQuery(e.target.value)}
          data-testid='secrets-search'
        />
      </InputGroup>
      {offered.length > 2 && (
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
    </div>
  );
};
