import { Bot } from "lucide-react";
import {
  useRevokeSandboxAgentToken,
  useSandboxAgentTokens
} from "@/hooks/api/sandboxAgentTokens/useSandboxAgentTokens";
import type { Token } from "@/types/apiToken";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import { RevokeTokenConfirm } from "../components/TokenList/RevokeTokenConfirm";
import { TokenListAsync } from "../components/TokenList/TokenListAsync";
import { TokenListSummary } from "../components/TokenList/TokenListSummary";
import { useRevokeConfirmation } from "../components/TokenList/useRevokeConfirmation";
import { useTokenTrailHref } from "../components/TokenList/useTokenTrailHref";
import { SandboxTokensTable } from "./components/SandboxTokensTable";
import { summarize } from "./tokenState";

const AREA = "admin-sandbox-tokens";

const DESCRIPTION = (
  <>
    Tokens that AI agents hold to build custom apps in a sandbox. Each reaches the sandboxes of the
    apps it names and nothing else, and expires within a week. You see the ones for apps in the
    organizations your staff access covers.
  </>
);

/** What a revoke confirmation says about the token: who minted it, and what stops. */
const revokeWarning = (token: Token | null): string => {
  const stops = "The agent holding it stops working at once, and the token can't be brought back.";
  return token?.owner.label ? `${token.owner.label} minted it. ${stops}` : stops;
};

/**
 * `/admin/sandbox-agent-tokens`: every staff member's sandbox agent tokens, newest first, with
 * revoke. The server gates it on `operate_platform` and so does the rail, so the refusal is for a
 * viewer whose access changed after the page loaded.
 */
export default function AdminSandboxAgentTokens() {
  const tokens = useSandboxAgentTokens();
  const revoke = useRevokeSandboxAgentToken();
  // A token's name links to its audit trail, for a viewer the audit log admits.
  const trailHref = useTokenTrailHref();
  const confirmation = useRevokeConfirmation(revoke.mutate);

  return (
    <AdminPage width='wide' description={DESCRIPTION} data-testid={AREA}>
      <TokenListAsync
        area={AREA}
        query={tokens}
        noun='sandbox agent tokens'
        refused='Seeing and revoking every staff member’s sandbox agent tokens needs the operate_platform capability. The tokens you minted yourself are in your account settings.'
        empty={
          <AdminEmptyState
            icon={Bot}
            title='No agent holds a token right now.'
            description='One appears here when an engineer approves an agent’s request, and stays listed after it expires or is revoked.'
            data-testid={`${AREA}-empty`}
          />
        }
      >
        {(rows) => (
          <>
            <TokenListSummary area={AREA} summary={summarize(rows)} />
            <SandboxTokensTable
              tokens={rows}
              revokingId={revoke.isPending ? (revoke.variables?.id ?? null) : null}
              onRevoke={confirmation.ask}
              trailHref={trailHref}
            />
          </>
        )}
      </TokenListAsync>

      <RevokeTokenConfirm area={AREA} confirmation={confirmation} warning={revokeWarning} />
    </AdminPage>
  );
}
