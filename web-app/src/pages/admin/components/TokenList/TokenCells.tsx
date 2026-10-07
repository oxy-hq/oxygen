import { Link } from "react-router-dom";
import {
  maskedToken,
  toTokenSummary
} from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/accessSummary";
import TokenStatus from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/components/TokenRow/components/TokenStatus";
import { Button } from "@/components/ui/shadcn/button";
import { TableCell } from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";
import { relativeTime } from "@/pages/admin/utils";
import { ApiKeyService } from "@/services/api/apiKey";
import type { Token } from "@/types/apiToken";
import { endedAt, tokenState } from "./tokenState";

/**
 * One line and one height a row, with or without a Revoke button: a cell is cut with an ellipsis
 * and says the whole of it on hover.
 */
export const TOKEN_CELL = "h-8 py-0";

interface CellProps {
  /** `admin-<area>`: the prefix of the cell's test id. */
  area: string;
  token: Token;
}

interface NameProps extends CellProps {
  /**
   * Where the audit log shows this token alone: what was done with it, and its own lifecycle.
   * Absent for a viewer who may not open the audit log.
   */
  trailHref?: string;
}

/**
 * The token's name. Its hover carries the prefix, which is how an audit row names the token. The
 * name leads to the audit trail whether the token works or not: what was done with it matters
 * after it stopped.
 */
export function TokenNameCell({ area, token, trailHref }: NameProps) {
  return (
    <TableCell
      className={cn(TOKEN_CELL, "truncate font-medium")}
      title={`${token.name}\n${maskedToken(token)}`}
      data-testid={`${area}-name`}
    >
      {trailHref ? (
        <Link
          to={trailHref}
          className='underline-offset-2 hover:underline focus-visible:underline'
          aria-label={`${token.name}: see what this token did in the audit log`}
          data-testid={`${area}-trail`}
        >
          {token.name}
        </Link>
      ) : (
        token.name
      )}
    </TableCell>
  );
}

/** Whose token it is, by the label the server gives: an email address. */
export function TokenOwnerCell({ token, testId }: { token: Token; testId: string }) {
  return (
    <TableCell
      className={cn(TOKEN_CELL, "truncate")}
      title={token.owner.label || undefined}
      data-testid={testId}
    >
      {token.owner.label || "Unknown"}
    </TableCell>
  );
}

/**
 * When the token dies, or how it ended and how long ago. The dot and the phrase are the account
 * list's own, so a token reads the same to staff as to its owner.
 */
export function TokenExpiryCell({ area, token }: CellProps) {
  const ended = endedAt(token);
  return (
    <TableCell className={cn(TOKEN_CELL, "truncate")} data-testid={`${area}-expiry`}>
      <TokenStatus token={toTokenSummary(token)} />
      {ended && (
        <span
          className='ml-1.5 tabular-nums'
          title={ApiKeyService.formatDate(ended)}
          data-testid={`${area}-ended`}
        >
          {relativeTime(ended)}
        </span>
      )}
    </TableCell>
  );
}

export function TokenLastUsedCell({ area, token }: CellProps) {
  return (
    <TableCell
      className={cn(TOKEN_CELL, "truncate text-muted-foreground tabular-nums")}
      title={token.last_used_at ? ApiKeyService.formatDate(token.last_used_at) : undefined}
      data-testid={`${area}-last-used`}
    >
      {relativeTime(token.last_used_at)}
    </TableCell>
  );
}

interface RevokeProps extends CellProps {
  /** This token's revoke is in flight. */
  revoking: boolean;
  onRevoke: (token: Token) => void;
}

/** Revoke, on a token that still works. An expired or revoked one offers nothing. */
export function TokenRevokeCell({ area, token, revoking, onRevoke }: RevokeProps) {
  return (
    <TableCell className={cn(TOKEN_CELL, "text-right")}>
      {tokenState(token) === "active" && (
        <Button
          variant='ghost'
          size='sm'
          // Red on hover and focus, not at rest: the row is about the token, not about ending it.
          className='h-6 px-2 font-medium text-foreground/80 text-xs! hover:bg-destructive/10 hover:text-destructive focus-visible:text-destructive'
          disabled={revoking}
          onClick={() => onRevoke(token)}
          aria-label={`Revoke ${token.name}`}
          data-testid={`${area}-revoke`}
        >
          {revoking ? "Revoking…" : "Revoke"}
        </Button>
      )}
    </TableCell>
  );
}
