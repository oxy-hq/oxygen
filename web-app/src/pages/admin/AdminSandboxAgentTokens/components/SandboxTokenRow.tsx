import AccessCell from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/components/TokenRow/components/AccessCell";
import { TableCell } from "@/components/ui/shadcn/table";
import {
  TOKEN_CELL,
  TokenExpiryCell,
  TokenLastUsedCell,
  TokenNameCell,
  TokenOwnerCell,
  TokenRevokeCell
} from "@/pages/admin/components/TokenList/TokenCells";
import { TokenListRow } from "@/pages/admin/components/TokenList/TokenListRow";
import type { Token } from "@/types/apiToken";
import { tokenState } from "../tokenState";

const AREA = "admin-sandbox-tokens";

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
 * One sandbox agent token: what it is called, who minted it, the apps it reaches, when it dies or
 * how it ended, and when it was last used. The apps and the expiry are the account list's own
 * cells, so a token reads the same to staff as to its minter: two apps by their `<org>/<app>`
 * reference, then a count, with every app and each grant on hover.
 *
 * Only a token that still works offers Revoke. An expired or revoked one is set back and offers
 * nothing. Its name still leads to its audit trail: what an agent did matters after it stopped.
 */
export function SandboxTokenRow({ token, revoking, onRevoke, trailHref }: Props) {
  const dead = tokenState(token) !== "active";

  return (
    <TokenListRow area={AREA} token={token}>
      <TokenNameCell area={AREA} token={token} trailHref={trailHref} />
      <TokenOwnerCell token={token} testId={`${AREA}-minter`} />
      <TableCell className={TOKEN_CELL} data-testid={`${AREA}-apps`}>
        <AccessCell token={token} quiet={dead} />
      </TableCell>
      <TokenExpiryCell area={AREA} token={token} />
      <TokenLastUsedCell area={AREA} token={token} />
      <TokenRevokeCell area={AREA} token={token} revoking={revoking} onRevoke={onRevoke} />
    </TokenListRow>
  );
}
