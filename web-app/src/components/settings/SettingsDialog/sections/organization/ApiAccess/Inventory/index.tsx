import { KeyRound, Search, SearchX } from "lucide-react";
import { useMemo, useState } from "react";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { searchText } from "@/components/settings/tokens/tokenFilter";
import { Button } from "@/components/ui/shadcn/button";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/shadcn/input-group";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useOrgTokens } from "@/hooks/api/orgApiAccess";
import type { InventoryFilters as Filters, InventoryToken } from "@/types/orgApiAccess";
import type { Organization } from "@/types/organization";
import { EmptyState, ListState } from "../shared/ListState";
import { InventoryFilters } from "./components/InventoryFilters";
import { InventoryRow } from "./components/InventoryRow";
import { LegacyKeyGroup } from "./components/LegacyKeyGroup";
import { hasActiveFilters, ownerOptions, showsTokenList, splitInventory } from "./inventory";

interface InventoryProps {
  org: Organization;
  /** Jumps to a service account's page; its tokens are managed there. */
  onOpenAccount: (accountId: string) => void;
}

/**
 * Every credential that reaches this organization, whoever holds it, in two lists that are never
 * mixed: API tokens (people's personal tokens, the org's own service-account tokens, and the
 * short-lived ones trusted access hands out), then the removed legacy API keys in a group of
 * their own. One read feeds both, so the controls above apply to both.
 *
 * The kind and owner filters are the server's — they are the query key, so the list is re-read.
 * The search box is not: it narrows what came back, by token, owner and app, which is the
 * question an org admin actually arrives with and is not worth a round trip.
 */
export function Inventory({ org, onOpenAccount }: InventoryProps) {
  const [filters, setFilters] = useState<Filters>({});
  const [query, setQuery] = useState("");
  const { data, isPending, error, refetch } = useOrgTokens(org.id, filters);
  // The owner menu lists owners from the *unfiltered* list, or picking an owner
  // would shrink the menu to that one owner. With no filter on, this is the
  // same cache entry as the read above, so it costs nothing extra.
  const { data: everyToken } = useOrgTokens(org.id);
  const all = data ?? [];
  const q = query.trim().toLowerCase();
  const rows = useMemo(
    () => (q ? all.filter((row) => searchText(row).includes(q)) : all),
    [all, q]
  );
  const { tokens, legacyKeys } = splitInventory(rows);
  const filtered = hasActiveFilters(filters) || q !== "";

  return (
    <div className='flex flex-col gap-4' data-testid='api-access-inventory'>
      <p className='max-w-xl text-muted-foreground text-xs leading-relaxed'>
        Every API token that can reach {org.name}, whoever holds it. You can end a personal token's
        access here without touching what it does elsewhere.
      </p>

      <div className='flex flex-col gap-2'>
        <InputGroup className='sm:max-w-xs'>
          <InputGroupAddon>
            <Search />
          </InputGroupAddon>
          <InputGroupInput
            placeholder='Search tokens, owners or apps'
            aria-label='Search the inventory'
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            data-testid='api-access-inventory-search'
          />
        </InputGroup>
        <InventoryFilters
          filters={filters}
          onChange={setFilters}
          owners={ownerOptions(everyToken ?? [])}
        />
      </div>

      <ListState
        what='tokens'
        isPending={isPending}
        // A failed background refetch keeps showing what was already loaded.
        error={data ? null : error}
        isEmpty={rows.length === 0}
        onRetry={refetch}
        testId='api-access-inventory'
        empty={
          filtered ? (
            <EmptyState
              icon={SearchX}
              title='Nothing matches'
              testId='api-access-inventory-no-match'
              action={
                <Button
                  size='sm'
                  variant='outline'
                  onClick={() => {
                    setFilters({});
                    setQuery("");
                  }}
                >
                  Clear search and filters
                </Button>
              }
            >
              Nothing that reaches this organization fits them.
            </EmptyState>
          ) : (
            <EmptyState
              icon={KeyRound}
              title='No tokens reach this organization'
              testId='api-access-inventory-empty'
            >
              Tokens appear here as soon as they can act in {org.name}: when a member creates one,
              when a service account gets one, or when a trusted workflow runs.
            </EmptyState>
          )
        }
      >
        {showsTokenList(filters) && (
          <TokenList org={org} tokens={tokens} filtered={filtered} onOpenAccount={onOpenAccount} />
        )}
        {legacyKeys.length > 0 && (
          <LegacyKeyGroup orgId={org.id} orgName={org.name} legacyKeys={legacyKeys} />
        )}
      </ListState>
    </div>
  );
}

interface TokenListProps {
  org: Organization;
  /** API tokens only: legacy API keys are listed in their own group. */
  tokens: InventoryToken[];
  filtered: boolean;
  onOpenAccount: (accountId: string) => void;
}

/** The API tokens that reach the org. When this is empty, only legacy keys ever did. */
function TokenList({ org, tokens, filtered, onOpenAccount }: TokenListProps) {
  if (tokens.length === 0) {
    return (
      <p
        className='rounded-lg border border-dashed px-4 py-3 text-muted-foreground text-xs'
        data-testid='api-access-inventory-no-tokens'
      >
        {filtered ? "No API token matches." : `No API tokens reach ${org.name} yet.`}
      </p>
    );
  }

  return (
    <TableWrapper>
      <Table className='text-xs' data-testid='api-access-inventory-table'>
        <TableHeader>
          <TableRow>
            <TableHead className='px-3'>Owner</TableHead>
            <TableHead className='px-3'>Token</TableHead>
            <TableHead className='px-3'>Kind</TableHead>
            <TableHead className='px-3'>Access here</TableHead>
            <TableHead className='px-3'>Status</TableHead>
            <TableHead className='px-3'>Last used</TableHead>
            <TableHead className='px-3'>Blocked by policy</TableHead>
            <TableHead className='px-3'>
              <span className='sr-only'>Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {tokens.map((token) => (
            <InventoryRow
              key={token.id}
              orgId={org.id}
              orgName={org.name}
              token={token}
              onOpenAccount={onOpenAccount}
            />
          ))}
        </TableBody>
      </Table>
    </TableWrapper>
  );
}
