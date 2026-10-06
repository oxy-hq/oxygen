import type React from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import type { TokenEndpoints } from "@/hooks/api/apiKeys/tokenEndpoints";
import { ApiKeyService } from "@/services/api/apiKey";
import type { TokenSummary } from "@/types/apiToken";
import ExtendApiKeyPopover from "./ExtendApiKeyPopover";

interface Props {
  token: TokenSummary;
  /** Omit on a read-only surface: an expired token then shows its badge with no Extend. */
  endpoints?: TokenEndpoints;
}

/**
 * Revoked / Expired / Active (with the countdown). An expired key carries its own Extend
 * button here, because extending is how a lapsed key comes back.
 */
const ApiKeyStatus: React.FC<Props> = ({ token, endpoints }) => {
  if (!token.is_active) {
    return <Badge variant='destructive'>Revoked</Badge>;
  }

  if (ApiKeyService.isExpired(token.expires_at)) {
    return (
      <div className='flex items-center gap-2'>
        <Badge variant='destructive'>Expired</Badge>
        {endpoints && (
          <ExtendApiKeyPopover token={token} endpoints={endpoints}>
            <Button
              variant='outline'
              size='sm'
              className='h-6 px-2 text-xs'
              data-testid='api-key-expired-extend-button'
            >
              Extend
            </Button>
          </ExtendApiKeyPopover>
        )}
      </div>
    );
  }

  const timeUntilExpiration = ApiKeyService.getTimeUntilExpiration(token.expires_at ?? undefined);
  if (timeUntilExpiration === null) {
    return <Badge variant='default'>Active</Badge>;
  }

  return (
    <div className='flex items-center gap-2'>
      <Badge variant='default'>Active</Badge>
      <span
        className='text-muted-foreground text-sm'
        title={token.expires_at ? ApiKeyService.formatDate(token.expires_at) : undefined}
        data-testid='api-key-expiry-countdown'
      >
        Expires in {timeUntilExpiration}
      </span>
    </div>
  );
};

export default ApiKeyStatus;
