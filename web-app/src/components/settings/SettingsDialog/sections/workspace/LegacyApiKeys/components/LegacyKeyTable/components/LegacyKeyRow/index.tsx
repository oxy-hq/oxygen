import { History } from "lucide-react";
import type React from "react";
import { useState } from "react";
import LegacyBadge from "@/components/settings/SettingsDialog/components/LegacyBadge";
import ActivityDrawer from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/components/ActivityDrawer";
import ApiKeyStatus from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/components/ApiKeyStatus";
import ExtendAction from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/components/ExtendAction";
import { formatLastUsed } from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/formatLastUsed";
import { Button } from "@/components/ui/shadcn/button";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import { useWorkspaceApiKeyEndpoints } from "@/hooks/api/apiKeys/tokenEndpoints";
import { useRevokeApiKey } from "@/hooks/api/apiKeys/useApiKeyMutations";
import { ApiKeyService } from "@/services/api/apiKey";
import type { ApiKey } from "@/types/apiKey";
import type { TokenSummary } from "@/types/apiToken";
import RevokeAction from "./components/RevokeAction";
import RevokeLegacyKeyDialog from "./components/RevokeLegacyKeyDialog";

interface Props {
  apiKey: ApiKey;
}

/**
 * One legacy API key: badged Legacy, with Activity, Extend and Revoke. The status badge, Extend
 * popover and Activity drawer are the ones the token surfaces use, pointed at the legacy routes.
 *
 * Activity and Extend are for whoever the list returned the key to: the server asks only that
 * the caller owns it. Revoke alone needs the workspace admin role (see RevokeAction).
 */
const LegacyKeyRow: React.FC<Props> = ({ apiKey }) => {
  const [isRevokeOpen, setIsRevokeOpen] = useState(false);
  const [isActivityOpen, setIsActivityOpen] = useState(false);

  const revoke = useRevokeApiKey();
  const endpoints = useWorkspaceApiKeyEndpoints();
  // The legacy list carries no `kind`. Stamping it here is what makes every shared piece say
  // "legacy API key" rather than guess from its absence.
  const summary: TokenSummary = { ...apiKey, kind: "legacy_key" };

  return (
    <TableRow data-testid='legacy-api-key-row' data-key-name={apiKey.name}>
      <TableCell data-label='Name'>
        <div className='flex items-center gap-2'>
          <span className='truncate font-medium'>{apiKey.name}</span>
          <LegacyBadge />
        </div>
        {apiKey.masked_key && (
          <div className='font-mono text-muted-foreground'>{apiKey.masked_key}</div>
        )}
      </TableCell>
      <TableCell data-label='Expiry'>
        <ApiKeyStatus token={summary} endpoints={endpoints} />
      </TableCell>
      <TableCell data-label='Last used'>{formatLastUsed(apiKey.last_used_at)}</TableCell>
      <TableCell data-label='Created'>{ApiKeyService.formatDate(apiKey.created_at)}</TableCell>
      <TableCell>
        <div className='flex items-center justify-end'>
          <Button
            variant='ghost'
            size='sm'
            onClick={() => setIsActivityOpen(true)}
            aria-label={`Activity for ${apiKey.name}`}
            title='Activity'
            data-testid='legacy-api-key-activity-button'
          >
            <History />
          </Button>
          <ExtendAction token={summary} endpoints={endpoints} />
          <RevokeAction
            name={apiKey.name}
            isActive={apiKey.is_active}
            isPending={revoke.isPending}
            onRevoke={() => setIsRevokeOpen(true)}
          />
        </div>
      </TableCell>
      <RevokeLegacyKeyDialog
        open={isRevokeOpen}
        onOpenChange={setIsRevokeOpen}
        name={apiKey.name}
        onConfirm={() => revoke.mutate(apiKey.id)}
      />
      <ActivityDrawer
        token={summary}
        endpoints={endpoints}
        open={isActivityOpen}
        onOpenChange={setIsActivityOpen}
      />
    </TableRow>
  );
};

export default LegacyKeyRow;
