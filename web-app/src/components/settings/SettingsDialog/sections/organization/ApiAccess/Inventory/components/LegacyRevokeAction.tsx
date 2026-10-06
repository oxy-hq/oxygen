import { Ban } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";

interface LegacyRevokeActionProps {
  /** The legacy API key's name, for the accessible name. */
  name: string;
  /** Why the org can't revoke it. */
  reason: string;
}

/**
 * Revoke, shown but disabled, on a legacy API key: only its owner can revoke one, and saying so
 * beats hiding the action. A disabled button swallows hover, so the tooltip hangs off a focusable
 * wrapper: the reason has to be reachable by mouse and by keyboard.
 */
export function LegacyRevokeAction({ name, reason }: LegacyRevokeActionProps) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        {/* biome-ignore lint/a11y/noNoninteractiveTabindex: makes the disabled action's reason keyboard-reachable */}
        <span tabIndex={0} className='inline-flex rounded-md focus-visible:outline-2'>
          <Button
            variant='ghost'
            size='icon'
            className='size-7 text-muted-foreground'
            disabled
            aria-label={`Revoke ${name}: ${reason}`}
            data-testid='api-access-inventory-revoke-legacy'
          >
            <Ban className='size-3.5' aria-hidden />
          </Button>
        </span>
      </TooltipTrigger>
      <TooltipContent side='left'>{reason}</TooltipContent>
    </Tooltip>
  );
}
