import { CalendarPlus } from "lucide-react";
import type React from "react";
import { Button } from "@/components/ui/shadcn/button";
import type { TokenEndpoints } from "@/hooks/api/apiKeys/tokenEndpoints";
import { ApiKeyService } from "@/services/api/apiKey";
import type { TokenSummary } from "@/types/apiToken";
import ExtendApiKeyPopover from "./ExtendApiKeyPopover";

/** A live token with an expiry: the only state the actions column offers Extend for. */
export const isExtendable = (token: TokenSummary): boolean =>
  token.is_active && !!token.expires_at && !ApiKeyService.isExpired(token.expires_at);

/**
 * Extend sits in the actions column for a live key that has an expiry. An expired key gets
 * its Extend in the status cell instead (see ApiKeyStatus); a revoked key or one that never
 * expires has nothing to extend.
 */
const ExtendAction: React.FC<{ token: TokenSummary; endpoints: TokenEndpoints }> = ({
  token,
  endpoints
}) => {
  if (!isExtendable(token)) {
    // Holds the column's width so the buttons after it stay aligned down the table.
    return <span className='inline-block h-8 w-10' aria-hidden />;
  }
  return (
    <ExtendApiKeyPopover token={token} endpoints={endpoints}>
      <Button
        variant='ghost'
        size='sm'
        aria-label={`Extend ${token.name}`}
        title='Extend'
        data-testid='api-key-extend-button'
      >
        <CalendarPlus />
      </Button>
    </ExtendApiKeyPopover>
  );
};

export default ExtendAction;
