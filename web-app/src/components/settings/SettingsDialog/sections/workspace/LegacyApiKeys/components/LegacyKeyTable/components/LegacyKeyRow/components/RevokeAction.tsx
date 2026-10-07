import { Trash2 } from "lucide-react";
import type React from "react";
import { CanWorkspaceAdmin } from "@/components/auth/Can";
import { Button } from "@/components/ui/shadcn/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";

/** Why Revoke is off for a key's owner who is not a workspace admin. */
const REVOKE_NEEDS_ADMIN = "Revoking a legacy API key needs a workspace admin role";

interface Props {
  /** The key's name, for the accessible name. */
  name: string;
  /** False once revoked: nothing is left to revoke, whoever asks. */
  isActive: boolean;
  isPending: boolean;
  onRevoke: () => void;
}

interface RevokeButtonProps {
  label: string;
  disabled: boolean;
  onClick?: () => void;
  /** Left off where a tooltip already says more than "Revoke". */
  title?: string;
}

const RevokeButton: React.FC<RevokeButtonProps> = ({ label, disabled, onClick, title }) => (
  <Button
    variant='ghost'
    size='sm'
    onClick={onClick}
    disabled={disabled}
    aria-label={label}
    title={title}
    data-testid='legacy-api-key-revoke-button'
  >
    <Trash2 className='!text-destructive' />
  </Button>
);

/**
 * Revoke for one legacy API key. The server lets a key's owner list, extend and inspect it, but
 * revoking needs the workspace admin role, so this is the one action in the row that is gated.
 * A non-admin sees it disabled with the reason rather than missing: they know the action exists
 * and who can take it. A disabled button swallows hover, so the tooltip hangs off a focusable
 * wrapper, reachable by mouse and by keyboard.
 */
const RevokeAction: React.FC<Props> = ({ name, isActive, isPending, onRevoke }) => {
  if (!isActive) return <RevokeButton label={`Revoke ${name}`} disabled title='Revoke' />;

  return (
    <CanWorkspaceAdmin
      fallback={
        <Tooltip>
          <TooltipTrigger asChild>
            {/* biome-ignore lint/a11y/noNoninteractiveTabindex: makes the disabled action's reason keyboard-reachable */}
            <span tabIndex={0} className='inline-flex rounded-md focus-visible:outline-2'>
              <RevokeButton label={`Revoke ${name}: ${REVOKE_NEEDS_ADMIN}`} disabled />
            </span>
          </TooltipTrigger>
          <TooltipContent side='left'>{REVOKE_NEEDS_ADMIN}</TooltipContent>
        </Tooltip>
      }
    >
      <RevokeButton
        label={`Revoke ${name}`}
        disabled={isPending}
        onClick={onRevoke}
        title='Revoke'
      />
    </CanWorkspaceAdmin>
  );
};

export default RevokeAction;
