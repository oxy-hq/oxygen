import type React from "react";
import { useState } from "react";
import ActivityDrawer from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/components/ActivityDrawer";
import { formatLastUsed } from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/formatLastUsed";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { USER_TOKEN_ENDPOINTS } from "@/hooks/api/apiKeys/tokenEndpoints";
import { AGENT_TOKEN_HINT, isAgentToken } from "@/libs/agentToken";
import { cn } from "@/libs/shadcn/utils";
import { ApiKeyService } from "@/services/api/apiKey";
import type { Token, TokenWithSecret } from "@/types/apiToken";
import { isFixedToken, tokenKindLabel, toTokenSummary } from "../../accessSummary";
import { TOKEN_COLUMNS } from "../tokenColumns";
import AccessCell from "./components/AccessCell";
import TokenName from "./components/TokenName";
import TokenRowActions from "./components/TokenRowActions";
import TokenStatus from "./components/TokenStatus";

interface Props {
  token: Token;
  onRegenerated: (regenerated: TokenWithSecret) => void;
}

/** One line, one height: a cell never wraps, and the row rule is the only box. */
const CELL = "py-0 pr-3 pl-0 md:h-12";

const WEEK_MS = 7 * 24 * 60 * 60 * 1000;

/**
 * "Never", "Today", "3 days ago", then the day alone: the column is narrow, and the hour of a use
 * that long ago is in the cell's title.
 */
const lastUsed = (at: string | null): string =>
  at && Date.now() - new Date(at).getTime() >= WEEK_MS
    ? ApiKeyService.formatDay(at)
    : formatLastUsed(at);

/**
 * One of the caller's personal access tokens, or a sandbox agent token they minted, on one line:
 * what it is called, its kind, the token, what it reaches, when it dies and when it was last
 * used. Activity and Extend are the pieces Workspace → Legacy API keys uses too, pointed at
 * `/user/tokens` here.
 *
 * A sandbox agent token is fixed once minted, so its row has no Rename, Extend, Edit access or
 * Regenerate: it shows what the token is, how long it has left, and Activity and Revoke.
 *
 * An agent token (`oxyc tokens create --agent`) is a personal token an agent holds for hours. Its
 * row says "Agent" where an `oxyc login` says "Personal", and it is fixed in the same way.
 */
const TokenRow: React.FC<Props> = ({ token, onRegenerated }) => {
  const [activityOpen, setActivityOpen] = useState(false);
  const summary = toTokenSummary(token);
  const fixed = isFixedToken(token);
  // Expired or revoked: the row is set back, since nothing can sign in with it.
  const dead = token.status !== "active";

  return (
    <TableRow
      className={cn("group/row hover:bg-transparent", dead && "text-muted-foreground")}
      data-testid='account-token-row'
      data-token-name={token.name}
      data-token-kind={token.kind}
      data-token-source={token.source}
      data-token-status={token.status}
    >
      {/* The title carries the token's prefix for a box too narrow for the Token column, under
          the name in full for one too long for its own. */}
      <TableCell
        data-label='Name'
        className={cn(CELL, "font-medium")}
        title={`${token.name}\n${summary.masked_key}`}
        data-testid='account-token-name-cell'
      >
        <TokenName token={token} editable={!fixed && summary.is_active} />
      </TableCell>
      <TableCell
        data-label='Kind'
        className={cn(CELL, TOKEN_COLUMNS.kind.shown, "truncate text-muted-foreground")}
      >
        <span
          title={
            isAgentToken(token)
              ? AGENT_TOKEN_HINT
              : fixed
                ? "For an AI agent building custom apps. It reaches the sandboxes of its apps and nothing else."
                : undefined
          }
          data-testid={fixed ? "account-token-kind-badge" : "account-token-kind"}
        >
          {tokenKindLabel(token)}
        </span>
      </TableCell>
      <TableCell data-label='Token' className={cn(CELL, TOKEN_COLUMNS.token.shown, "truncate")}>
        <span className='font-mono text-muted-foreground' data-testid='account-token-masked'>
          {summary.masked_key}
        </span>
      </TableCell>
      <TableCell data-label='Access' className={CELL}>
        <AccessCell token={token} quiet={dead} />
      </TableCell>
      <TableCell data-label='Expiry' className={cn(CELL, "truncate")}>
        <TokenStatus token={summary} />
      </TableCell>
      <TableCell
        data-label='Last used'
        className={cn(CELL, TOKEN_COLUMNS.lastUsed.shown, "truncate text-muted-foreground")}
        title={token.last_used_at ? ApiKeyService.formatDate(token.last_used_at) : undefined}
      >
        {lastUsed(token.last_used_at)}
      </TableCell>
      <TableCell className={cn(CELL, "pr-0")}>
        <TokenRowActions
          token={token}
          summary={summary}
          onActivity={() => setActivityOpen(true)}
          onRegenerated={onRegenerated}
        />
      </TableCell>
      <ActivityDrawer
        token={summary}
        endpoints={USER_TOKEN_ENDPOINTS}
        open={activityOpen}
        onOpenChange={setActivityOpen}
      />
    </TableRow>
  );
};

export default TokenRow;
