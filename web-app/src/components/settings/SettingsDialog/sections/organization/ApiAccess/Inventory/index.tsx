import { KeyRound, SearchX } from "lucide-react";
import { useState } from "react";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Button } from "@/components/ui/shadcn/button";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useOrgTokens } from "@/hooks/api/orgApiAccess";
import type { InventoryFilters as Filters, InventoryToken } from "@/types/orgApiAccess";
import type { Organization } from "@/types/organization";
import { EmptyState, ListState } from "../shared/ListState";
import { usePickableWorkspaces } from "../shared/usePickableLists";
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
 * short-lived ones trusted access hands out), then legacy API keys in a group of their own.
 * One read feeds both, so the filters above apply to both.
 */
export function Inventory({ org, onOpenAccount }: InventoryProps) {
  const [filters, setFilters] = useState<Filters>({});
  const { data, isPending, error, refetch } = useOrgTokens(org.id, filters);
  // The owner menu lists owners from the *unfiltered* list, or picking an owner
  // would shrink the menu to that one owner. With no filter on, this is the
  // same cache entry as the read above, so it costs nothing extra.
  const { data: everyToken } = useOrgTokens(org.id);
  const workspaces = usePickableWorkspaces(org.id);
  const rows = data ?? [];
  const { tokens, legacyKeys } = splitInventory(rows);
  const filtered = hasActiveFilters(filters);

  return (
    <div className='flex flex-col gap-4' data-testid='api-access-inventory'>
      <p className='max-w-xl text-muted-foreground text-xs leading-relaxed'>
        Every API token that can reach {org.name}, whoever holds it. You can end a personal token's
        access here without touching what it does elsewhere.
      </p>

      <InventoryFilters
        filters={filters}
        onChange={setFilters}
        owners={ownerOptions(everyToken ?? [])}
        workspaces={workspaces.items}
      />

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
              title='Nothing matches these filters'
              testId='api-access-inventory-no-match'
              action={
                <Button size='sm' variant='outline' onClick={() => setFilters({})}>
                  Clear filters
                </Button>
              }
            >
              Nothing that reaches this organization fits them. Clear the filters to see everything.
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

/** The API tokens that reach the org. Only legacy API keys did, when this is empty. */
function TokenList({ org, tokens, filtered, onOpenAccount }: TokenListProps) {
  if (tokens.length === 0) {
    return (
      <p
        className='rounded-lg border border-dashed px-4 py-3 text-muted-foreground text-xs'
        data-testid='api-access-inventory-no-tokens'
      >
        {filtered ? "No API tokens match these filters." : `No API tokens reach ${org.name} yet.`}
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
