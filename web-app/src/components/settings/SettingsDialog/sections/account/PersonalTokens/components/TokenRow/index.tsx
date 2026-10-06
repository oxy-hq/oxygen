import { History } from "lucide-react";
import type React from "react";
import { useState } from "react";
import ActivityDrawer from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/components/ActivityDrawer";
import ApiKeyStatus from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/components/ApiKeyStatus";
import ExtendAction from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/components/ExtendAction";
import { formatLastUsed } from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/formatLastUsed";
import { Button } from "@/components/ui/shadcn/button";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { USER_TOKEN_ENDPOINTS } from "@/hooks/api/apiKeys/tokenEndpoints";
import type { Token, TokenWithSecret } from "@/types/apiToken";
import { toTokenSummary } from "../../accessSummary";
import AccessCell from "./components/AccessCell";
import TokenName from "./components/TokenName";
import TokenRowMenu from "./components/TokenRowMenu";

interface Props {
  token: Token;
  onRegenerated: (regenerated: TokenWithSecret) => void;
}

/**
 * One of the caller's personal access tokens. Status, Extend and Activity are the components
 * Workspace → Legacy API keys uses too, pointed at `/user/tokens` here.
 */
const TokenRow: React.FC<Props> = ({ token, onRegenerated }) => {
  const [activityOpen, setActivityOpen] = useState(false);
  const summary = toTokenSummary(token);

  return (
    <TableRow
      className='group/row'
      data-testid='account-token-row'
      data-token-name={token.name}
      data-token-kind={token.kind}
      data-token-status={token.status}
    >
      <TableCell data-label='Name' className='font-medium'>
        <TokenName token={token} editable={summary.is_active} />
      </TableCell>
      <TableCell data-label='Token'>
        <span className='font-mono text-muted-foreground' data-testid='account-token-masked'>
          {summary.masked_key}
        </span>
      </TableCell>
      <TableCell data-label='Access'>
        <AccessCell token={token} />
      </TableCell>
      <TableCell data-label='Expiry'>
        <ApiKeyStatus token={summary} endpoints={USER_TOKEN_ENDPOINTS} />
      </TableCell>
      <TableCell data-label='Last used'>{formatLastUsed(token.last_used_at)}</TableCell>
      <TableCell>
        <div className='flex items-center justify-end'>
          <Button
            variant='ghost'
            size='sm'
            onClick={() => setActivityOpen(true)}
            aria-label={`Activity for ${token.name}`}
            title='Activity'
            data-testid='account-token-activity-button'
          >
            <History />
          </Button>
          <ExtendAction token={summary} endpoints={USER_TOKEN_ENDPOINTS} />
          {/* A revoked token has nothing left to edit, regenerate or revoke. */}
          {summary.is_active ? (
            <TokenRowMenu token={token} onRegenerated={onRegenerated} />
          ) : (
            <span className='inline-block h-8 w-10' aria-hidden />
          )}
        </div>
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
