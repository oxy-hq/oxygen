import { Ban, CalendarPlus, History, RefreshCw } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { toTokenSummary } from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/accessSummary";
import ActivityDrawer from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/components/ActivityDrawer";
import ExtendApiKeyPopover from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/components/ExtendApiKeyPopover";
import { Button } from "@/components/ui/shadcn/button";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import {
  useRegenerateServiceAccountToken,
  useRevokeServiceAccountToken,
  useServiceAccountTokenEndpoints
} from "@/hooks/api/orgApiAccess";
import { cn } from "@/libs/shadcn/utils";
import { exceedsPolicyMessage } from "@/libs/tokenPolicy";
import type { Token, TokenWithSecret } from "@/types/apiToken";
import { AccessSummary } from "../../../shared/AccessSummary";
import { ConfirmDialog } from "../../../shared/ConfirmDialog";
import { TokenLifecycleCell } from "../../../shared/TokenLifecycleCell";
import { describeApiError } from "../../../utils/errors";
import { describeAccess } from "../../../utils/grants";
import { isExtendable, isRegenerable, isRevocable, lastUsedText } from "../../../utils/tokens";

interface TokenRowProps {
  orgId: string;
  saId: string;
  token: Token;
  allowNoExpiry: boolean;
  /** Hands the new secret up, so the pane can show it once. */
  onRegenerated: (secret: TokenWithSecret) => void;
}

const CELL = "px-3 py-2.5 align-top max-md:px-0 max-md:py-0";
const ICON_BUTTON = "size-7 text-muted-foreground";

export function TokenRow({ orgId, saId, token, allowNoExpiry, onRegenerated }: TokenRowProps) {
  const ref = { orgId, saId, tokenId: token.id };
  // Extend and Activity are the components every token surface shares, pointed at this account.
  const endpoints = useServiceAccountTokenEndpoints(orgId, saId);
  const summary = toTokenSummary(token);
  const regenerate = useRegenerateServiceAccountToken();
  const revoke = useRevokeServiceAccountToken();
  const [activityOpen, setActivityOpen] = useState(false);
  const [confirming, setConfirming] = useState<"regenerate" | "revoke" | null>(null);

  const handleRegenerate = async () => {
    try {
      const secret = await regenerate.mutateAsync(ref);
      setConfirming(null);
      onRegenerated(secret);
    } catch (err) {
      toast.error(
        exceedsPolicyMessage(err, "regenerate") ??
          describeApiError(err, `Couldn't regenerate ${token.name}.`)
      );
    }
  };

  const handleRevoke = async () => {
    try {
      await revoke.mutateAsync(ref);
      toast.success(`Revoked ${token.name}`);
      setConfirming(null);
    } catch (err) {
      toast.error(describeApiError(err, `Couldn't revoke ${token.name}.`));
    }
  };

  return (
    <TableRow
      className={cn(token.status === "revoked" && "text-muted-foreground")}
      data-testid='api-access-token-row'
      data-token-name={token.name}
    >
      <TableCell data-label='Name' className={cn(CELL, "whitespace-normal")}>
        <p className='font-medium text-foreground'>{token.name}</p>
        <p className='font-mono text-muted-foreground'>{summary.masked_key}</p>
      </TableCell>
      <TableCell data-label='Access' className={cn(CELL, "whitespace-normal")}>
        <AccessSummary access={describeAccess(token.grants)} />
      </TableCell>
      <TableCell data-label='Status' className={CELL}>
        <TokenLifecycleCell token={token} testId='api-access-token-status' />
      </TableCell>
      <TableCell data-label='Last used' className={CELL}>
        {lastUsedText(token.last_used_at)}
      </TableCell>
      <TableCell className={cn(CELL, "text-right")}>
        <div className='flex items-center justify-end gap-0.5'>
          <Button
            variant='ghost'
            size='icon'
            className={ICON_BUTTON}
            onClick={() => setActivityOpen(true)}
            aria-label={`Activity for ${token.name}`}
            title='Activity'
            data-testid='api-access-token-activity'
          >
            <History className='size-3.5' aria-hidden />
          </Button>
          {isExtendable(token) && (
            <ExtendApiKeyPopover
              token={summary}
              endpoints={endpoints}
              allowNoExpiry={allowNoExpiry}
            >
              <Button
                variant='ghost'
                size='icon'
                className={ICON_BUTTON}
                aria-label={`Extend ${token.name}`}
                title='Extend'
                data-testid='api-access-token-extend'
              >
                <CalendarPlus className='size-3.5' aria-hidden />
              </Button>
            </ExtendApiKeyPopover>
          )}
          {isRegenerable(token) && (
            <Button
              variant='ghost'
              size='icon'
              className={ICON_BUTTON}
              onClick={() => setConfirming("regenerate")}
              aria-label={`Regenerate ${token.name}`}
              title='Regenerate'
              data-testid='api-access-token-regenerate'
            >
              <RefreshCw className='size-3.5' aria-hidden />
            </Button>
          )}
          {isRevocable(token) && (
            <Button
              variant='ghost'
              size='icon'
              className={cn(ICON_BUTTON, "hover:text-destructive")}
              onClick={() => setConfirming("revoke")}
              aria-label={`Revoke ${token.name}`}
              title='Revoke'
              data-testid='api-access-token-revoke'
            >
              <Ban className='size-3.5' aria-hidden />
            </Button>
          )}
        </div>

        <ConfirmDialog
          open={confirming === "regenerate"}
          onOpenChange={(open) => !open && setConfirming(null)}
          title={`Regenerate ${token.name}?`}
          confirmLabel='Regenerate'
          cancelLabel='Keep current secret'
          isPending={regenerate.isPending}
          onConfirm={handleRegenerate}
          testId='api-access-token-regenerate-dialog'
        >
          <p>
            The current secret stops working at once, and you get a new one to put everywhere the
            old one is deployed. The token keeps its name, its access and its expiry.
          </p>
          <p>If you only need more time, extend it instead: that keeps the secret.</p>
        </ConfirmDialog>

        <ConfirmDialog
          open={confirming === "revoke"}
          onOpenChange={(open) => !open && setConfirming(null)}
          title={`Revoke ${token.name}?`}
          confirmLabel='Revoke'
          cancelLabel='Keep token'
          destructive
          isPending={revoke.isPending}
          onConfirm={handleRevoke}
          testId='api-access-token-revoke-dialog'
        >
          <p>
            Anything still using this token fails from that moment. This can't be undone; to give
            access back you create a new token.
          </p>
        </ConfirmDialog>

        <ActivityDrawer
          token={summary}
          endpoints={endpoints}
          open={activityOpen}
          onOpenChange={setActivityOpen}
        />
      </TableCell>
    </TableRow>
  );
}
