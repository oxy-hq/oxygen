import type React from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { cn } from "@/libs/shadcn/utils";

/** What the badge says on hover: the one fact that sets a legacy API key apart from a token. */
const LEGACY_BADGE_HINT =
  "A legacy API key reaches everything its owner can. It can't be limited to workspaces.";

/**
 * Marks a row as a legacy API key, wherever one is listed. Every legacy row carries it, so a
 * legacy key is never read as an API token.
 */
const LegacyBadge: React.FC<{ className?: string }> = ({ className }) => (
  <Badge
    variant='outline'
    className={cn("shrink-0 font-normal text-muted-foreground", className)}
    title={LEGACY_BADGE_HINT}
    data-testid='legacy-badge'
  >
    Legacy
  </Badge>
);

export default LegacyBadge;
