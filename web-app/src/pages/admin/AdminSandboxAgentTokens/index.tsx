import { Bot, ShieldOff } from "lucide-react";
import { useState } from "react";
import ConfirmDialog from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/components/TokenRow/components/ConfirmDialog";
import {
  useRevokeSandboxAgentToken,
  useSandboxAgentTokens
} from "@/hooks/api/sandboxAgentTokens/useSandboxAgentTokens";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { apiStatus } from "@/libs/apiError";
import ROUTES from "@/libs/utils/routes";
import type { Token } from "@/types/apiToken";
import { navItemReachable } from "../AdminLayout/adminNav";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import { SandboxTokensTable } from "./components/SandboxTokensTable";
import { summarize } from "./tokenState";

const DESCRIPTION = (
  <>
    Tokens that AI agents hold to build custom apps in a sandbox. Each reaches the sandboxes of the
    apps it names and nothing else, and expires within a week. You see the ones for apps in the
    organizations your staff access covers.
  </>
);

/** The audit log narrowed to one token: what was done with it, and its own lifecycle. */
const auditTrailHref = (token: Token): string => `${ROUTES.ADMIN.AUDIT}?token_id=${token.id}`;

/** What a revoke confirmation says about the token: who minted it, and what stops. */
const revokeWarning = (token: Token | null): string => {
  const stops = "The agent holding it stops working at once, and the token can't be brought back.";
  return token?.owner.label ? `${token.owner.label} minted it. ${stops}` : stops;
};

/**
 * `/admin/sandbox-agent-tokens`: every staff member's sandbox agent tokens, newest first, with
 * revoke. The server gates it on `operate_platform` and so does the rail, so the refusal below is
 * for a viewer whose access changed after the page loaded.
 */
export default function AdminSandboxAgentTokens() {
  // The whole query, so a failed fetch and a list with nothing in it never read the same.
  const tokens = useSandboxAgentTokens();
  const revoke = useRevokeSandboxAgentToken();
  // A token's name links to its audit trail, for a viewer the audit log admits. The rail asks
  // the same question of the same map, so the link never leads to a page that bounces.
  const { data: user } = useCurrentUser();
  const seesAudit = navItemReachable(ROUTES.ADMIN.AUDIT, {
    isOwner: user?.is_owner ?? false,
    capabilities: user?.platform_capabilities ?? []
  });
  // Kept after the dialog closes, so its title does not blank while it fades out.
  const [target, setTarget] = useState<Token | null>(null);
  const [confirming, setConfirming] = useState(false);

  const askRevoke = (token: Token) => {
    setTarget(token);
    setConfirming(true);
  };

  const confirmRevoke = () => {
    if (target) revoke.mutate({ id: target.id, name: target.name });
  };

  return (
    <AdminPage width='wide' description={DESCRIPTION} data-testid='admin-sandbox-tokens'>
      {apiStatus(tokens.error) === 403 ? (
        <AdminEmptyState
          icon={ShieldOff}
          title="Your staff access doesn't include this list."
          description='Seeing and revoking every staff member’s sandbox agent tokens needs the operate_platform capability. The tokens you minted yourself are in your account settings.'
          data-testid='admin-sandbox-tokens-refused'
        />
      ) : (
        <AdminAsync
          query={tokens}
          noun='sandbox agent tokens'
          rows={4}
          isEmpty={(rows) => rows.length === 0}
          empty={
            <AdminEmptyState
              icon={Bot}
              title='No agent holds a token right now.'
              description='One appears here when an engineer approves an agent’s request, and stays listed after it expires or is revoked.'
              data-testid='admin-sandbox-tokens-empty'
            />
          }
        >
          {(rows) => {
            const summary = summarize(rows);
            return (
              <>
                <p
                  className='text-muted-foreground text-xs'
                  data-testid='admin-sandbox-tokens-summary'
                >
                  <span className='font-medium text-foreground'>{summary.lead}</span>
                  {summary.rest}
                </p>
                <SandboxTokensTable
                  tokens={rows}
                  revokingId={revoke.isPending ? (revoke.variables?.id ?? null) : null}
                  onRevoke={askRevoke}
                  trailHref={seesAudit ? auditTrailHref : undefined}
                />
              </>
            );
          }}
        </AdminAsync>
      )}

      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title={`Revoke ${target?.name ?? "this token"}?`}
        description={revokeWarning(target)}
        action='Revoke'
        testId='admin-sandbox-tokens-revoke-confirm'
        onConfirm={confirmRevoke}
      />
    </AdminPage>
  );
}
