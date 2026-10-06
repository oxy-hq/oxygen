import type React from "react";
import TableContentWrapper from "@/components/settings/components/TableContentWrapper";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useUserTokens } from "@/hooks/api/userTokens/useUserTokens";
import { apiStatus } from "@/libs/apiError";
import { cn } from "@/libs/shadcn/utils";
import type { TokenWithSecret } from "@/types/apiToken";
import TokenRow from "./TokenRow";
import { ACTIONS_COLUMN_WIDTH, TOKEN_COLUMNS } from "./tokenColumns";

interface Props {
  onRegenerated: (regenerated: TokenWithSecret) => void;
}

const loadError = (error: Error | null): string | undefined => {
  if (!error) return undefined;
  // The route isn't there: an older server. Nothing the person can retry their way out of.
  return apiStatus(error) === 404
    ? "Personal access tokens aren't available on this server yet."
    : "Couldn't load your tokens.";
};

const COLUMNS = Object.values(TOKEN_COLUMNS);
const HEAD = "h-9 pr-3 pl-0 font-medium text-muted-foreground";

/**
 * The caller's personal access tokens, newest first, one line each under a hairline. No legacy
 * API keys: the route omits them.
 *
 * The table is as wide as its box and no wider, so the actions are never scrolled out of sight:
 * its layout is fixed, Access takes what the other columns leave, and `tokenColumns` says which
 * columns give way in a narrow box.
 *
 * The list is this one request. A sandbox agent token's apps are named by their `<org>/<app>`
 * reference, which each grant carries itself, so nothing else is read to show a row.
 */
const TokenTable: React.FC<Props> = ({ onRegenerated }) => {
  const { data, isLoading, error, refetch } = useUserTokens();
  const tokens = data?.tokens ?? [];

  return (
    <TableWrapper plain>
      <Table
        className='min-w-120 table-fixed text-xs'
        containerClassName='@container'
        data-testid='account-token-table'
      >
        <TableHeader>
          <TableRow className='hover:bg-transparent'>
            {COLUMNS.map((column) => (
              <TableHead key={column.label} className={cn(HEAD, column.width, column.shown)}>
                {column.label}
              </TableHead>
            ))}
            <TableHead className={cn(HEAD, ACTIONS_COLUMN_WIDTH, "pr-0")}>
              <span className='sr-only'>Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody className='[&_tr:last-child]:border-b'>
          <TableContentWrapper
            isEmpty={tokens.length === 0}
            loading={isLoading}
            colSpan={COLUMNS.length + 1}
            error={loadError(error)}
            noFoundTitle='No tokens yet'
            noFoundDescription='Create one to use oxyc, a script or CI as yourself.'
            onRetry={apiStatus(error) === 404 ? undefined : refetch}
          >
            {tokens.map((token) => (
              <TokenRow key={token.id} token={token} onRegenerated={onRegenerated} />
            ))}
          </TableContentWrapper>
        </TableBody>
      </Table>
    </TableWrapper>
  );
};

export default TokenTable;
