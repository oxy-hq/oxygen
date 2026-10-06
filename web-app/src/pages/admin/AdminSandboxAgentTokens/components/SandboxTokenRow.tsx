import { Link } from "react-router-dom";
import { toTokenSummary } from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/accessSummary";
import AccessCell from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/components/TokenRow/components/AccessCell";
import TokenStatus from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/components/TokenRow/components/TokenStatus";
import { Button } from "@/components/ui/shadcn/button";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";
import { relativeTime } from "@/pages/admin/utils";
import { ApiKeyService } from "@/services/api/apiKey";
import type { Token } from "@/types/apiToken";
import { endedAt, tokenState } from "../tokenState";

/**
 * One line and one height a row, with or without a Revoke button: a cell is cut with an ellipsis
 * and says the whole of it on hover.
 */
const CELL = "h-8 py-0";

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
  const state = tokenState(token);
  const dead = state !== "active";
  const summary = toTokenSummary(token);
  const ended = endedAt(token);

  return (
    <TableRow
      className={cn("border-border/50", dead && "text-muted-foreground")}
      data-testid='admin-sandbox-tokens-row'
      data-token-id={token.id}
      data-token-name={token.name}
      data-token-status={state}
    >
      {/* The title carries the token's prefix, which is how an audit row names it. */}
      <TableCell
        className={cn(CELL, "truncate font-medium")}
        title={`${token.name}\n${summary.masked_key}`}
        data-testid='admin-sandbox-tokens-name'
      >
        {trailHref ? (
          <Link
            to={trailHref}
            className='underline-offset-2 hover:underline focus-visible:underline'
            aria-label={`${token.name}: see what this token did in the audit log`}
            data-testid='admin-sandbox-tokens-trail'
          >
            {token.name}
          </Link>
        ) : (
          token.name
        )}
      </TableCell>
      <TableCell
        className={cn(CELL, "truncate")}
        title={token.owner.label || undefined}
        data-testid='admin-sandbox-tokens-minter'
      >
        {token.owner.label || "Unknown"}
      </TableCell>
      <TableCell className={CELL} data-testid='admin-sandbox-tokens-apps'>
        <AccessCell token={token} quiet={dead} />
      </TableCell>
      <TableCell className={cn(CELL, "truncate")} data-testid='admin-sandbox-tokens-expiry'>
        <TokenStatus token={summary} />
        {ended && (
          <span
            className='ml-1.5 tabular-nums'
            title={ApiKeyService.formatDate(ended)}
            data-testid='admin-sandbox-tokens-ended'
          >
            {relativeTime(ended)}
          </span>
        )}
      </TableCell>
      <TableCell
        className={cn(CELL, "truncate text-muted-foreground tabular-nums")}
        title={token.last_used_at ? ApiKeyService.formatDate(token.last_used_at) : undefined}
        data-testid='admin-sandbox-tokens-last-used'
      >
        {relativeTime(token.last_used_at)}
      </TableCell>
      <TableCell className={cn(CELL, "text-right")}>
        {!dead && (
          <Button
            variant='ghost'
            size='sm'
            // Red on hover and focus, not at rest: the row is about the token, not about ending it.
            className='h-6 px-2 font-medium text-foreground/80 text-xs! hover:bg-destructive/10 hover:text-destructive focus-visible:text-destructive'
            disabled={revoking}
            onClick={() => onRevoke(token)}
            aria-label={`Revoke ${token.name}`}
            data-testid='admin-sandbox-tokens-revoke'
          >
            {revoking ? "Revoking…" : "Revoke"}
          </Button>
        )}
      </TableCell>
    </TableRow>
  );
}
