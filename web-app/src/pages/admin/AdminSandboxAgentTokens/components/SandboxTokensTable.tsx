import {
  type TokenListColumn,
  TokenListTable
} from "@/pages/admin/components/TokenList/TokenListTable";
import { SANDBOX_AGENT_TOKEN_LIST_LIMIT } from "@/services/api/sandboxAgentTokens";
import type { Token } from "@/types/apiToken";
import { SandboxTokenRow } from "./SandboxTokenRow";

/** Apps takes what the other columns leave and cuts with an ellipsis. */
const COLUMNS: TokenListColumn[] = [
  { label: "Token", className: "w-44" },
  { label: "Minted by", className: "w-48" },
  { label: "Apps" },
  { label: "Expiry", className: "w-36" },
  { label: "Last used", className: "w-24" },
  { label: "Actions", className: "w-24", align: "right", srOnly: true }
];

interface Props {
  /** Newest first, as the server sent them. Never empty: the page shows that case itself. */
  tokens: Token[];
  /** The token whose revoke is in flight, if any. */
  revokingId: string | null;
  onRevoke: (token: Token) => void;
  /** Where the audit log shows one token alone. Absent for a viewer who may not open it. */
  trailHref?: (token: Token) => string;
}

/** The tokens, in the server's order. */
export function SandboxTokensTable({ tokens, revokingId, onRevoke, trailHref }: Props) {
  return (
    <TokenListTable
      area='admin-sandbox-tokens'
      columns={COLUMNS}
      className='min-w-160'
      fetched={tokens.length}
      limit={SANDBOX_AGENT_TOKEN_LIST_LIMIT}
    >
      {tokens.map((token) => (
        <SandboxTokenRow
          key={token.id}
          token={token}
          revoking={revokingId === token.id}
          onRevoke={onRevoke}
          trailHref={trailHref?.(token)}
        />
      ))}
    </TokenListTable>
  );
}
