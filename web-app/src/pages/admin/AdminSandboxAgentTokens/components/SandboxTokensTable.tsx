import { Table, TableBody, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import { SANDBOX_AGENT_TOKEN_LIST_LIMIT } from "@/services/api/sandboxAgentTokens";
import type { Token } from "@/types/apiToken";
import { SandboxTokenRow } from "./SandboxTokenRow";

interface Props {
  /** Newest first, as the server sent them. Never empty: the page shows that case itself. */
  tokens: Token[];
  /** The token whose revoke is in flight, if any. */
  revokingId: string | null;
  onRevoke: (token: Token) => void;
  /** Where the audit log shows one token alone. Absent for a viewer who may not open it. */
  trailHref?: (token: Token) => string;
}

/**
 * The tokens, in the server's order. The layout is fixed: Apps takes what the other columns
 * leave and cuts with an ellipsis, so Revoke is never scrolled out of sight.
 */
export function SandboxTokensTable({ tokens, revokingId, onRevoke, trailHref }: Props) {
  return (
    <div className='space-y-2'>
      <div className='overflow-x-auto rounded-md border border-border/60'>
        <Table className='min-w-160 table-fixed text-xs' data-testid='admin-sandbox-tokens-table'>
          <TableHeader>
            <TableRow className={ADMIN_HEADER_ROW_CLASS}>
              <AdminTh className='w-44'>Token</AdminTh>
              <AdminTh className='w-48'>Minted by</AdminTh>
              <AdminTh>Apps</AdminTh>
              <AdminTh className='w-36'>Expiry</AdminTh>
              <AdminTh className='w-24'>Last used</AdminTh>
              <AdminTh align='right' className='w-24'>
                <span className='sr-only'>Actions</span>
              </AdminTh>
            </TableRow>
          </TableHeader>
          <TableBody>
            {tokens.map((token) => (
              <SandboxTokenRow
                key={token.id}
                token={token}
                revoking={revokingId === token.id}
                onRevoke={onRevoke}
                trailHref={trailHref?.(token)}
              />
            ))}
          </TableBody>
        </Table>
      </div>
      {tokens.length >= SANDBOX_AGENT_TOKEN_LIST_LIMIT && (
        <p className='text-muted-foreground text-xs' data-testid='admin-sandbox-tokens-truncated'>
          Showing the newest {SANDBOX_AGENT_TOKEN_LIST_LIMIT}. Older tokens are not listed.
        </p>
      )}
    </div>
  );
}
