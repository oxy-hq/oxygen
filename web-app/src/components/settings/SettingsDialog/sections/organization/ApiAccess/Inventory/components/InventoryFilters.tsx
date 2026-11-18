import { Button } from "@/components/ui/shadcn/button";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import type { TokenKind, TokenOwner } from "@/types/apiToken";
import type { InventoryFilters as Filters } from "@/types/orgApiAccess";
import { KIND_LABELS } from "../../utils/tokens";
import { hasActiveFilters, withFilter } from "../inventory";

/** Radix Select has no empty value, so "no filter" needs a name of its own. */
const ALL = "__all__";
const KINDS: TokenKind[] = ["personal", "legacy_key", "service_account", "ci"];

interface InventoryFiltersProps {
  filters: Filters;
  onChange: (next: Filters) => void;
  owners: TokenOwner[];
}

/**
 * Kind and owner. There was a workspace filter too; each org has one workspace now and no screen
 * names it, so the list is the organization's whole and the filter went.
 */
export function InventoryFilters({ filters, onChange, owners }: InventoryFiltersProps) {
  const people = owners.filter((o) => o.type === "user");
  const accounts = owners.filter((o) => o.type === "service_account");
  const pick = <K extends keyof Filters>(key: K, value: string) =>
    onChange(withFilter(filters, key, value === ALL ? undefined : (value as Filters[K])));

  return (
    <div className='flex flex-wrap items-center gap-2' data-testid='api-access-inventory-filters'>
      <Select value={filters.kind ?? ALL} onValueChange={(v) => pick("kind", v)}>
        <SelectTrigger
          size='sm'
          className='h-8 w-40 text-xs'
          aria-label='Filter by kind'
          data-testid='api-access-inventory-filter-kind'
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={ALL} className='text-xs'>
            All kinds
          </SelectItem>
          {KINDS.map((kind) => (
            <SelectItem key={kind} value={kind} className='text-xs'>
              {KIND_LABELS[kind]}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>

      <Select value={filters.owner ?? ALL} onValueChange={(v) => pick("owner", v)}>
        <SelectTrigger
          size='sm'
          className='h-8 w-44 text-xs'
          aria-label='Filter by owner'
          data-testid='api-access-inventory-filter-owner'
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={ALL} className='text-xs'>
            All owners
          </SelectItem>
          <OwnerGroup label='People' owners={people} />
          <OwnerGroup label='Service accounts' owners={accounts} />
        </SelectContent>
      </Select>

      {hasActiveFilters(filters) && (
        <Button
          variant='ghost'
          size='sm'
          className='h-8 px-2 text-muted-foreground text-xs'
          onClick={() => onChange({})}
          data-testid='api-access-inventory-filter-clear'
        >
          Clear filters
        </Button>
      )}
    </div>
  );
}

function OwnerGroup({ label, owners }: { label: string; owners: TokenOwner[] }) {
  if (owners.length === 0) return null;
  return (
    <SelectGroup>
      <SelectLabel className='text-xs'>{label}</SelectLabel>
      {owners.map((owner) => (
        <SelectItem key={owner.id} value={owner.id} className='text-xs'>
          {owner.label}
        </SelectItem>
      ))}
    </SelectGroup>
  );
}
