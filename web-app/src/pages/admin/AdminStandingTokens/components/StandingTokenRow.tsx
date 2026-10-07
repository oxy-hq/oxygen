import AccessCell from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/components/TokenRow/components/AccessCell";
import { TableCell } from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";
import {
  TOKEN_CELL,
  TokenExpiryCell,
  TokenLastUsedCell,
  TokenNameCell,
  TokenOwnerCell,
  TokenRevokeCell
} from "@/pages/admin/components/TokenList/TokenCells";
import { TokenListRow } from "@/pages/admin/components/TokenList/TokenListRow";
import { tokenState } from "@/pages/admin/components/TokenList/tokenState";
import type { Token } from "@/types/apiToken";
import { fromOxycLogin, madeWith, standingHint, standingLabel } from "../standing";

const AREA = "admin-standing-tokens";

interface Props {
  token: Token;
  /** This token's revoke is in flight. */
  revoking: boolean;
  onRevoke: (token: Token) => void;
  /**
   * Where the audit log shows this token alone: what was done with it, and its own lifecycle.
   * Absent for a viewer who may not open the audit log.
   */
  trailHref?: string;
}

/**
 * One token that carries standing: what it is called, whose it is, the standing it carries, what
 * it reaches, how it was made, when it dies or how it ended, and when it was last used. Reach is
 * the account list's own cell, so a token reads the same to staff as to its owner, minus the
 * standing, which has its column here. "All access" on a token that works is set in weight: it
 * is the one cell that says how much a lost token gives away.
 *
 * Only a token that still works offers Revoke. An expired or revoked one is set back and offers
 * nothing, and its name still leads to its audit trail.
 */
export function StandingTokenRow({ token, revoking, onRevoke, trailHref }: Props) {
  const dead = tokenState(token) !== "active";
  const made = madeWith(token);

  return (
    <TokenListRow
      area={AREA}
      token={token}
      facts={{
        "data-token-staff": String(token.platform),
        "data-token-partner": String(token.partner),
        "data-token-all-access": String(token.all_access),
        "data-token-oxyc-login": String(fromOxycLogin(token))
      }}
    >
      <TokenNameCell area={AREA} token={token} trailHref={trailHref} />
      <TokenOwnerCell token={token} testId={`${AREA}-owner`} />
      <TableCell
        className={cn(TOKEN_CELL, "truncate")}
        title={standingHint(token)}
        data-testid={`${AREA}-standing`}
      >
        {standingLabel(token)}
      </TableCell>
      <TableCell
        className={cn(TOKEN_CELL, !dead && token.all_access && "font-medium")}
        data-testid={`${AREA}-reach`}
      >
        <AccessCell token={token} quiet={dead} owner='its owner' withStanding={false} />
      </TableCell>
      <TableCell
        className={cn(TOKEN_CELL, "truncate")}
        title={made.hint}
        data-testid={`${AREA}-made-with`}
      >
        {made.label}
      </TableCell>
      <TokenExpiryCell area={AREA} token={token} />
      <TokenLastUsedCell area={AREA} token={token} />
      <TokenRevokeCell area={AREA} token={token} revoking={revoking} onRevoke={onRevoke} />
    </TokenListRow>
  );
}
