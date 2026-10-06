import { MoreHorizontal } from "lucide-react";
import React, { useState } from "react";
import ExtendApiKeyPopover from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/components/ExtendApiKeyPopover";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { USER_TOKEN_ENDPOINTS } from "@/hooks/api/apiKeys/tokenEndpoints";
import {
  useRegenerateUserToken,
  useRevokeUserToken
} from "@/hooks/api/userTokens/useUserTokenMutations";
import { cn } from "@/libs/shadcn/utils";
import { ApiKeyService } from "@/services/api/apiKey";
import type { Token, TokenSummary, TokenWithSecret } from "@/types/apiToken";
import { isFixedToken } from "../../../accessSummary";
import EditAccessDialog from "../../EditAccessDialog";
import ConfirmDialog from "./ConfirmDialog";

interface Props {
  token: Token;
  summary: TokenSummary;
  onActivity: () => void;
  /** Handed the one response that carries the new secret. */
  onRegenerated: (regenerated: TokenWithSecret) => void;
}

type ActionProps = React.ComponentProps<typeof Button> & {
  /** Turns red on hover and on focus, not at rest: the row is not about revoking. */
  destructive?: boolean;
};

/** An action named by its word. */
const RowAction = React.forwardRef<HTMLButtonElement, ActionProps>(
  ({ destructive, className, ...props }, ref) => (
    <Button
      ref={ref}
      variant='ghost'
      size='sm'
      className={cn(
        "h-7 w-full px-1.5 font-medium text-foreground/80 text-xs! hover:text-foreground",
        destructive &&
          "hover:bg-destructive/10 hover:text-destructive focus-visible:text-destructive",
        className
      )}
      {...props}
    />
  )
);
RowAction.displayName = "RowAction";

/** One action's place. Kept when the action isn't offered, so each lines up down the table. */
const Slot: React.FC<React.PropsWithChildren<{ className: string }>> = ({
  className,
  children
}) => <span className={cn("inline-flex shrink-0 justify-center", className)}>{children}</span>;

/**
 * What can be done to one token, as words in fixed places: Extend, Activity and Revoke, then a
 * menu for Edit access and Regenerate.
 *
 * A token with an expiry can be extended while it is live and after it lapses, since extending
 * is how a lapsed one comes back. A sandbox agent token is fixed once minted: it has Activity
 * and Revoke, and nothing else. A revoked token keeps Activity alone.
 */
const TokenRowActions: React.FC<Props> = ({ token, summary, onActivity, onRegenerated }) => {
  const [editOpen, setEditOpen] = useState(false);
  const [regenerateOpen, setRegenerateOpen] = useState(false);
  const [revokeOpen, setRevokeOpen] = useState(false);
  const regenerate = useRegenerateUserToken();
  const revoke = useRevokeUserToken();
  const fixed = isFixedToken(token);
  const live = summary.is_active;
  const busy = regenerate.isPending || revoke.isPending;
  const expired = ApiKeyService.isExpired(summary.expires_at);

  return (
    <div className='flex items-center justify-end gap-0.5'>
      <Slot className='w-13'>
        {live && !fixed && summary.expires_at && (
          <ExtendApiKeyPopover token={summary} endpoints={USER_TOKEN_ENDPOINTS}>
            <RowAction
              aria-label={`Extend ${token.name}`}
              data-testid={expired ? "api-key-expired-extend-button" : "api-key-extend-button"}
            >
              Extend
            </RowAction>
          </ExtendApiKeyPopover>
        )}
      </Slot>
      <Slot className='w-14'>
        <RowAction
          onClick={onActivity}
          aria-label={`Activity for ${token.name}`}
          data-testid='account-token-activity-button'
        >
          Activity
        </RowAction>
      </Slot>
      <Slot className='w-13.5'>
        {live && (
          <RowAction
            destructive
            disabled={busy}
            onClick={() => setRevokeOpen(true)}
            aria-label={`Revoke ${token.name}`}
            data-testid='account-token-revoke'
          >
            Revoke
          </RowAction>
        )}
      </Slot>
      <Slot className='w-6'>
        {live && !fixed && (
          // `modal={false}`: a modal menu that opens a dialog leaves `pointer-events: none` on body.
          <DropdownMenu modal={false}>
            <DropdownMenuTrigger asChild>
              <Button
                variant='ghost'
                size='sm'
                className='size-6 p-0 text-foreground/80 data-[state=open]:bg-muted'
                disabled={busy}
                aria-label={`More actions for ${token.name}`}
                data-testid='account-token-menu-button'
              >
                <MoreHorizontal />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align='end' className='w-44'>
              <DropdownMenuItem
                onClick={() => setEditOpen(true)}
                data-testid='account-token-edit-access'
              >
                Edit access…
              </DropdownMenuItem>
              <DropdownMenuItem
                onClick={() => setRegenerateOpen(true)}
                data-testid='account-token-regenerate'
              >
                Regenerate…
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </Slot>

      {!fixed && (
        <>
          <EditAccessDialog token={token} open={editOpen} onOpenChange={setEditOpen} />
          <ConfirmDialog
            open={regenerateOpen}
            onOpenChange={setRegenerateOpen}
            title={`Regenerate ${token.name}?`}
            description='The current secret stops working at once. You get a new one with the same access and expiry, to put wherever this token is used.'
            action='Regenerate'
            testId='account-token-regenerate-confirm'
            onConfirm={() => regenerate.mutate(token.id, { onSuccess: onRegenerated })}
          />
        </>
      )}
      <ConfirmDialog
        open={revokeOpen}
        onOpenChange={setRevokeOpen}
        title={`Revoke ${token.name}?`}
        description="Anything using it stops working at once, and it can't be brought back."
        action='Revoke'
        testId='account-token-revoke-confirm'
        onConfirm={() => revoke.mutate({ id: token.id, name: token.name })}
      />
    </div>
  );
};

export default TokenRowActions;
