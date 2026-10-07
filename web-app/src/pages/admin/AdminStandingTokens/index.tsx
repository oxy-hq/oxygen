import { KeyRound } from "lucide-react";
import { useState } from "react";
import {
  useRevokeStandingToken,
  useStandingTokens
} from "@/hooks/api/standingTokens/useStandingTokens";
import { apiErrorCode } from "@/libs/apiError";
import type { Token } from "@/types/apiToken";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import { RevokeTokenConfirm } from "../components/TokenList/RevokeTokenConfirm";
import { TokenListAsync } from "../components/TokenList/TokenListAsync";
import { TokenListSummary } from "../components/TokenList/TokenListSummary";
import { useRevokeConfirmation } from "../components/TokenList/useRevokeConfirmation";
import { useTokenTrailHref } from "../components/TokenList/useTokenTrailHref";
import { StandingTokenFilters } from "./components/StandingTokenFilters";
import { StandingTokensTable } from "./components/StandingTokensTable";
import { summarize } from "./summary";
import {
  filterCounts,
  filterTokens,
  NO_FILTER,
  noMatchMessage,
  type TokenFilter
} from "./tokenFilter";

const AREA = "admin-standing-tokens";

const DESCRIPTION = (
  <>
    Personal API tokens that act with their owner’s Oxygen staff standing, or reach a partner’s
    client organizations. Running oxyc login makes one that reaches everything its owner can. Revoke
    one here when a laptop is lost or a token has leaked. These tokens work across organizations, so
    this list is for staff whose access covers every organization.
  </>
);

/** What a revoke confirmation says: whose token it is, and what happens next. */
const revokeWarning = (token: Token | null): string => {
  const owner = token?.owner.label;
  return owner
    ? `${owner} owns it. It stops working at once, and they sign in or run oxyc login again to get a new one.`
    : "It stops working at once, and its owner signs in or runs oxyc login again to get a new one.";
};

const REFUSED_NO_CAPABILITY =
  "Seeing and revoking other people’s staff and partner tokens needs the manage_platform_grants capability. Your own tokens are in your account settings.";
const REFUSED_BOUNDED =
  "Your staff access is limited to some organizations. A token with staff or partner standing works across all of them, so seeing and revoking these needs access to every organization. Your own tokens are in your account settings.";

/** Why the server refused: a grant bounded to some organizations, or no capability at all. */
const refusal = (error: unknown): string =>
  apiErrorCode(error) === "unbounded_grant_required" ? REFUSED_BOUNDED : REFUSED_NO_CAPABILITY;

/**
 * `/admin/standing-tokens`: every personal token that carries staff or partner standing, newest
 * first, with revoke. The server gates it on `manage_platform_grants` held over every
 * organization. The rail shows the page to anyone with the capability and cannot tell a bounded
 * grant from an unbounded one, so the refusal is what a bounded viewer sees.
 */
export default function AdminStandingTokens() {
  const tokens = useStandingTokens();
  const revoke = useRevokeStandingToken();
  // A token's name links to its audit trail, for a viewer the audit log admits.
  const trailHref = useTokenTrailHref();
  const confirmation = useRevokeConfirmation(revoke.mutate);
  // Narrowed here, not by the server: the list is at most a few hundred rows.
  const [filter, setFilter] = useState<TokenFilter>(NO_FILTER);

  return (
    <AdminPage width='wide' description={DESCRIPTION} data-testid={AREA}>
      <TokenListAsync
        area={AREA}
        query={tokens}
        noun='staff and partner tokens'
        refused={refusal(tokens.error)}
        empty={
          <AdminEmptyState
            icon={KeyRound}
            title='No token carries staff or partner standing.'
            description='One appears here when a staff member or a partner runs oxyc login, or makes a token that includes their standing. It stays listed after it expires or is revoked.'
            data-testid={`${AREA}-empty`}
          />
        }
      >
        {(rows) => (
          <div className='space-y-3'>
            <TokenListSummary area={AREA} summary={summarize(rows)} />
            <StandingTokenFilters
              filter={filter}
              counts={filterCounts(rows, filter)}
              onChange={setFilter}
            />
            <StandingTokensTable
              tokens={filterTokens(rows, filter)}
              fetched={rows.length}
              noMatch={noMatchMessage(filter)}
              onShowAll={() => setFilter(NO_FILTER)}
              revokingId={revoke.isPending ? (revoke.variables?.id ?? null) : null}
              onRevoke={confirmation.ask}
              trailHref={trailHref}
            />
          </div>
        )}
      </TokenListAsync>

      <RevokeTokenConfirm area={AREA} confirmation={confirmation} warning={revokeWarning} />
    </AdminPage>
  );
}
