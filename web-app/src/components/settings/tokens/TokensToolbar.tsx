import { Search } from "lucide-react";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/shadcn/input-group";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import { FILTER_LABELS, type TokenFilter } from "./tokenFilter";

const ORDER: TokenFilter[] = ["all", "active", "expiring", "ended"];

interface TokensToolbarProps {
  query: string;
  onQuery: (query: string) => void;
  filter: TokenFilter;
  onFilter: (filter: TokenFilter) => void;
  counts: Record<TokenFilter, number>;
  /** `settings-<area>` — each surface's rows and controls share one prefix. */
  testIdPrefix: string;
}

/**
 * Search and filter above a token list, the same shape the secrets table uses:
 * one box on the left, the chips that narrow it on the right, each carrying
 * the count it would leave.
 *
 * A chip that would show nothing is not offered — a filter you can only
 * discover is empty by clicking it is worse than no filter — but the one in use
 * always stays, so it can be cleared.
 */
export const TokensToolbar = ({
  query,
  onQuery,
  filter,
  onFilter,
  counts,
  testIdPrefix
}: TokensToolbarProps) => {
  const offered = ORDER.filter((f) => f === "all" || f === filter || counts[f] > 0);
  return (
    <div className='flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between'>
      <InputGroup className='sm:max-w-xs'>
        <InputGroupAddon>
          <Search />
        </InputGroupAddon>
        <InputGroupInput
          placeholder='Search tokens, apps or workspaces'
          aria-label='Search tokens'
          value={query}
          onChange={(e) => onQuery(e.target.value)}
          data-testid={`${testIdPrefix}-search`}
        />
      </InputGroup>
      {offered.length > 2 && (
        <ToggleGroup
          type='single'
          size='sm'
          variant='outline'
          value={filter}
          onValueChange={(value) => onFilter((value || "all") as TokenFilter)}
          aria-label='Filter tokens'
          className='flex-wrap justify-start'
        >
          {offered.map((f) => (
            <ToggleGroupItem
              key={f}
              value={f}
              className='whitespace-nowrap px-2.5 text-xs'
              data-testid={`${testIdPrefix}-filter-${f}`}
            >
              {FILTER_LABELS[f]}
              <span className='ml-1 text-muted-foreground tabular-nums'>{counts[f]}</span>
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      )}
    </div>
  );
};
