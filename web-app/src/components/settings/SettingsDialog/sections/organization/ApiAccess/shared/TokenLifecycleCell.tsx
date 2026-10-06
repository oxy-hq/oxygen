import { Badge } from "@/components/ui/shadcn/badge";
import { cn } from "@/libs/shadcn/utils";
import type { Token } from "@/types/apiToken";
import { formatDay, type TokenTone, tokenLifecycle } from "../utils/tokens";

/**
 * Colour is spent on the states that need a decision. A healthy token is
 * plain; one about to lapse is amber; one that no longer works is muted or
 * red depending on whether it can come back (expired can, revoked can't).
 */
const BADGE_TONES: Record<TokenTone, string> = {
  active: "border-primary/30 bg-primary/5 text-primary",
  soon: "border-warning/40 bg-warning/10 text-foreground",
  expired: "border-destructive/30 bg-destructive/5 text-destructive",
  revoked: "text-muted-foreground"
};

/** Active / Expired / Revoked, and the one line that says when. */
export function TokenLifecycleCell({
  token,
  testId
}: {
  token: Pick<Token, "status" | "expires_at" | "revoked_at">;
  testId?: string;
}) {
  const life = tokenLifecycle(token);
  return (
    <div className='flex flex-col items-start gap-1' data-testid={testId} data-tone={life.tone}>
      <Badge variant='outline' className={cn("font-medium", BADGE_TONES[life.tone])}>
        {life.label}
      </Badge>
      <span
        className={cn(
          "text-muted-foreground text-xs tabular-nums",
          life.tone === "soon" && "text-foreground"
        )}
        title={token.expires_at ? formatDay(token.expires_at) : undefined}
      >
        {life.detail}
      </span>
    </div>
  );
}
