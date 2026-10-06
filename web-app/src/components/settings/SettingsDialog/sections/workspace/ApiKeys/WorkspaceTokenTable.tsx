import type React from "react";
import TableContentWrapper from "@/components/settings/components/TableContentWrapper";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Badge } from "@/components/ui/shadcn/badge";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import useWorkspaceTokens from "@/hooks/api/userTokens/useWorkspaceTokens";
import { apiStatus } from "@/libs/apiError";
import type { WorkspaceTokenRow } from "@/types/apiToken";
import {
  CEILING_LABELS,
  TOKEN_KIND_LABELS,
  toTokenSummary
} from "../../account/PersonalTokens/accessSummary";
import ApiKeyStatus from "./ApiKeyTable/ApiKeyRow/components/ApiKeyStatus";
import { formatLastUsed } from "./ApiKeyTable/ApiKeyRow/formatLastUsed";

const loadError = (error: Error | null): string | undefined => {
  if (!error) return undefined;
  // The route isn't there: an older server. Nothing the person can retry their way out of.
  return apiStatus(error) === 404
    ? "The token inventory isn't available on this server yet."
    : "Couldn't load the tokens that reach this workspace.";
};

/**
 * What the token may do in this workspace. A sandbox agent token is listed because the apps it
 * builds live here, and whatever ceiling it is listed with, the sandboxes of those apps are all
 * it reaches: showing the ceiling would read as far more than that.
 */
const AccessHere: React.FC<{ token: WorkspaceTokenRow }> = ({ token }) => {
  if (token.kind === "sandbox_agent") {
    return (
      <span
        title='Held by an AI agent: it can build and test sandboxes of the apps it names, and nothing else in this workspace.'
        data-testid='workspace-token-ceiling'
      >
        App sandboxes only
      </span>
    );
  }
  return (
    <span
      title={
        token.all_access ? "An all-access token: here it can do whatever its owner can." : undefined
      }
      data-testid='workspace-token-ceiling'
    >
      {CEILING_LABELS[token.role_ceiling_here] ?? token.role_ceiling_here}
    </span>
  );
};

/** One token that can act here. Nothing on the row changes it: its owner does that. */
const InventoryRow: React.FC<{ token: WorkspaceTokenRow }> = ({ token }) => {
  const summary = toTokenSummary(token);
  return (
    <TableRow
      data-testid='workspace-token-row'
      data-token-name={token.name}
      data-token-kind={token.kind}
    >
      <TableCell data-label='Name'>
        <div className='font-medium'>{token.name}</div>
        <div className='font-mono text-muted-foreground'>{summary.masked_key}</div>
      </TableCell>
      <TableCell data-label='Owner'>{token.owner.label}</TableCell>
      <TableCell data-label='Kind'>
        <Badge variant='outline' className='font-normal'>
          {TOKEN_KIND_LABELS[token.kind] ?? token.kind}
        </Badge>
      </TableCell>
      <TableCell data-label='Access here'>
        <AccessHere token={token} />
      </TableCell>
      <TableCell data-label='Expiry'>
        {/* No endpoints: a read-only surface, so an expired token shows no Extend. */}
        <ApiKeyStatus token={summary} />
      </TableCell>
      <TableCell data-label='Last used'>{formatLastUsed(token.last_used_at)}</TableCell>
    </TableRow>
  );
};

/**
 * Every API token that can reach this workspace, whoever owns it. Read-only. The route returns
 * no legacy API keys: those are listed under Workspace → Legacy API keys.
 */
const WorkspaceTokenTable: React.FC = () => {
  const { data, isLoading, error, refetch } = useWorkspaceTokens();
  const tokens = data?.tokens ?? [];

  return (
    <TableWrapper>
      <Table className='text-xs' data-testid='workspace-tokens-table'>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Owner</TableHead>
            <TableHead>Kind</TableHead>
            <TableHead>Access here</TableHead>
            <TableHead>Expiry</TableHead>
            <TableHead>Last used</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableContentWrapper
            isEmpty={tokens.length === 0}
            loading={isLoading}
            colSpan={6}
            error={loadError(error)}
            noFoundTitle='No tokens reach this workspace'
            noFoundDescription='A token appears here once someone creates one that covers it.'
            onRetry={apiStatus(error) === 404 ? undefined : refetch}
          >
            {tokens.map((token) => (
              <InventoryRow key={token.id} token={token} />
            ))}
          </TableContentWrapper>
        </TableBody>
      </Table>
    </TableWrapper>
  );
};

export default WorkspaceTokenTable;
