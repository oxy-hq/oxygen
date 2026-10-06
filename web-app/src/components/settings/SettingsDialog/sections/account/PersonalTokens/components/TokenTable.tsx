import type React from "react";
import TableContentWrapper from "@/components/settings/components/TableContentWrapper";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useUserTokens } from "@/hooks/api/userTokens/useUserTokens";
import { apiStatus } from "@/libs/apiError";
import type { TokenWithSecret } from "@/types/apiToken";
import TokenRow from "./TokenRow";

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

/** The caller's personal access tokens, newest first. No legacy API keys: the route omits them. */
const TokenTable: React.FC<Props> = ({ onRegenerated }) => {
  const { data, isLoading, error, refetch } = useUserTokens();
  const tokens = data?.tokens ?? [];

  return (
    <TableWrapper>
      <Table className='text-xs' data-testid='account-token-table'>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Token</TableHead>
            <TableHead>Access</TableHead>
            <TableHead>Expiry</TableHead>
            <TableHead>Last used</TableHead>
            <TableHead>
              <span className='sr-only'>Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableContentWrapper
            isEmpty={tokens.length === 0}
            loading={isLoading}
            colSpan={6}
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
