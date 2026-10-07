import { Button } from "@/components/ui/shadcn/button";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import {
  type TokenListColumn,
  TokenListTable
} from "@/pages/admin/components/TokenList/TokenListTable";
import { STANDING_TOKEN_LIST_LIMIT } from "@/services/api/standingTokens";
import type { Token } from "@/types/apiToken";
import { StandingTokenRow } from "./StandingTokenRow";

/**
 * Token, Owner and Reaches share what the fixed columns leave, and each cuts with an ellipsis, so
 * the eight columns hold the console's width without a sideways scroll.
 */
const COLUMNS: TokenListColumn[] = [
  { label: "Token" },
  { label: "Owner" },
  { label: "Standing", className: "w-30" },
  { label: "Reaches" },
  { label: "Made with", className: "w-22" },
  { label: "Expiry", className: "w-36" },
  { label: "Last used", className: "w-20" },
  { label: "Actions", className: "w-24", align: "right", srOnly: true }
];

interface Props {
  /** The rows the filter left, newest first as the server sent them. */
  tokens: Token[];
  /** How many the server sent before any filter: what the limit note is about. */
  fetched: number;
  /** What to say when the filter left no row. */
  noMatch: string;
  onShowAll: () => void;
  /** The token whose revoke is in flight, if any. */
  revokingId: string | null;
  onRevoke: (token: Token) => void;
  /** Where the audit log shows one token alone. Absent for a viewer who may not open it. */
  trailHref?: (token: Token) => string;
}

/**
 * The tokens, in the server's order. A filter that leaves nothing keeps the frame and says so in
 * it, with the way back to the whole list.
 */
export function StandingTokensTable({
  tokens,
  fetched,
  noMatch,
  onShowAll,
  revokingId,
  onRevoke,
  trailHref
}: Props) {
  return (
    <TokenListTable
      area='admin-standing-tokens'
      columns={COLUMNS}
      className='min-w-225'
      fetched={fetched}
      limit={STANDING_TOKEN_LIST_LIMIT}
    >
      {tokens.length === 0 ? (
        <TableRow className='hover:bg-transparent' data-testid='admin-standing-tokens-no-match'>
          <TableCell colSpan={COLUMNS.length} className='h-20 text-center text-muted-foreground'>
            {noMatch}
            <Button
              variant='link'
              size='sm'
              className='ml-1 h-auto p-0 text-xs!'
              onClick={onShowAll}
              data-testid='admin-standing-tokens-show-all'
            >
              Show every token
            </Button>
          </TableCell>
        </TableRow>
      ) : (
        tokens.map((token) => (
          <StandingTokenRow
            key={token.id}
            token={token}
            revoking={revokingId === token.id}
            onRevoke={onRevoke}
            trailHref={trailHref?.(token)}
          />
        ))
      )}
    </TokenListTable>
  );
}
