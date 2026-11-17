import AccessCell from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/components/TokenRow/components/AccessCell";
import { Button } from "@/components/ui/shadcn/button";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
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
import {
  type TokenListColumn,
  TokenListTable
} from "@/pages/admin/components/TokenList/TokenListTable";
import { relativeTime } from "@/pages/admin/utils";
import { TokenExpiry } from "@/services/api/tokenExpiry";
import type { Token } from "@/types/apiToken";
import type { PublishToken } from "@/types/publishTokens";
import { fromOxycLogin, standingHint, standingLabel } from "../standing";
import { kindInfo, type TokenRow } from "../tokenRows";

export const AREA = "admin-tokens";

const COLUMNS: TokenListColumn[] = [
  { label: "Token", className: "w-[22%]" },
  { label: "Kind", className: "w-[12%]" },
  { label: "Owner", className: "w-[18%]" },
  { label: "Reach" },
  { label: "Expires", className: "w-[13%]" },
  { label: "Last used", className: "w-[10%]" },
  { label: "Actions", className: "w-[9%]", align: "right", srOnly: true }
];

/**
 * Every token on one table, whatever minted it. The columns are what the three kinds
 * share — who holds it, what it reaches, when it stops — and the Kind column says which
 * of the three it is, so a lost laptop's staff token and a CI pipeline's publish token
 * are found in the same place.
 */
export function TokensTable({
  rows,
  fetched,
  limit,
  revokingKey,
  onRevoke,
  trailHref
}: {
  rows: TokenRow[];
  /** How many rows the server returned before filtering — for the truncation note. */
  fetched: number;
  limit: number;
  revokingKey: string | null;
  onRevoke: (row: TokenRow) => void;
  trailHref?: (token: Token) => string;
}) {
  return (
    <TokenListTable area={AREA} columns={COLUMNS} fetched={fetched} limit={limit}>
      {rows.map((row) => {
        const key = `${row.kind}:${row.id}`;
        const revoking = revokingKey === key;
        return row.kind === "publish" ? (
          <PublishRow
            key={key}
            token={row.token}
            revoking={revoking}
            onRevoke={() => onRevoke(row)}
          />
        ) : (
          <TokenListRow
            key={key}
            area={AREA}
            token={row.token}
            facts={
              row.kind === "standing"
                ? {
                    "data-token-kind": row.kind,
                    "data-token-staff": String(row.token.platform),
                    "data-token-partner": String(row.token.partner),
                    "data-token-all-access": String(row.token.all_access),
                    "data-token-oxyc-login": String(fromOxycLogin(row.token))
                  }
                : { "data-token-kind": row.kind }
            }
          >
            <TokenNameCell area={AREA} token={row.token} trailHref={trailHref?.(row.token)} />
            <KindCell kind={row.kind} />
            <TokenOwnerCell token={row.token} testId={`${AREA}-owner`} />
            {row.kind === "standing" ? (
              <TableCell
                className={cn(TOKEN_CELL, "truncate")}
                title={standingHint(row.token)}
                data-testid={`${AREA}-reach`}
              >
                {standingLabel(row.token)}
              </TableCell>
            ) : (
              <TableCell className={TOKEN_CELL} data-testid={`${AREA}-reach`}>
                <AccessCell token={row.token} quiet={!row.live} />
              </TableCell>
            )}
            <TokenExpiryCell area={AREA} token={row.token} />
            <TokenLastUsedCell area={AREA} token={row.token} />
            <TokenRevokeCell
              area={AREA}
              token={row.token}
              revoking={revoking}
              onRevoke={() => onRevoke(row)}
            />
          </TokenListRow>
        );
      })}
    </TokenListTable>
  );
}

const KindCell = ({ kind }: { kind: TokenRow["kind"] }) => (
  <TableCell
    className={cn(TOKEN_CELL, "truncate text-muted-foreground")}
    data-testid={`${AREA}-kind`}
  >
    {kindInfo(kind).label}
  </TableCell>
);

/**
 * A publish token in the same columns. It has no expiry and no per-app grant: it acts as
 * its minting admin on the custom-apps publish surface, for any app, until revoked.
 */
function PublishRow({
  token,
  revoking,
  onRevoke
}: {
  token: PublishToken;
  revoking: boolean;
  onRevoke: () => void;
}) {
  return (
    <TableRow
      className={cn("border-border/50", token.revoked && "text-muted-foreground")}
      data-testid={`${AREA}-row`}
      data-token-id={token.id}
      data-token-name={token.name}
      data-token-kind='publish'
      data-token-status={token.revoked ? "revoked" : "active"}
    >
      <TableCell
        className={cn(TOKEN_CELL, "truncate font-medium")}
        title={`${token.name}\n${token.token_prefix}…`}
        data-testid={`${AREA}-name`}
      >
        {token.name}
        <span className='ml-1.5 font-mono font-normal text-[10px] text-muted-foreground'>
          {token.token_prefix}…
        </span>
      </TableCell>
      <KindCell kind='publish' />
      <TableCell
        className={cn(TOKEN_CELL, "truncate")}
        title={token.created_by}
        data-testid={`${AREA}-owner`}
      >
        {token.created_by}
      </TableCell>
      <TableCell className={cn(TOKEN_CELL, "truncate")} data-testid={`${AREA}-reach`}>
        Publishes any custom app
      </TableCell>
      <TableCell className={cn(TOKEN_CELL, "truncate")} data-testid={`${AREA}-expiry`}>
        {token.revoked ? (
          <span>
            Revoked
            {token.revoked_at ? (
              <span
                className='ml-1.5 tabular-nums'
                title={TokenExpiry.formatDate(token.revoked_at)}
              >
                {relativeTime(token.revoked_at)}
              </span>
            ) : null}
          </span>
        ) : (
          <span className='text-muted-foreground'>Never</span>
        )}
      </TableCell>
      <TableCell
        className={cn(TOKEN_CELL, "truncate text-muted-foreground tabular-nums")}
        title={token.last_used_at ? TokenExpiry.formatDate(token.last_used_at) : undefined}
        data-testid={`${AREA}-last-used`}
      >
        {relativeTime(token.last_used_at)}
      </TableCell>
      <TableCell className={cn(TOKEN_CELL, "text-right")}>
        {!token.revoked && (
          <Button
            variant='ghost'
            size='sm'
            className='h-6 px-2 font-medium text-foreground/80 text-xs! hover:bg-destructive/10 hover:text-destructive focus-visible:text-destructive'
            disabled={revoking}
            onClick={onRevoke}
            aria-label={`Revoke ${token.name}`}
            data-testid={`${AREA}-revoke`}
          >
            {revoking ? "Revoking…" : "Revoke"}
          </Button>
        )}
      </TableCell>
    </TableRow>
  );
}
