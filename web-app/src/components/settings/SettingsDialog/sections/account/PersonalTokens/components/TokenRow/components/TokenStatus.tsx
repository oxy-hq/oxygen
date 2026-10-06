import type React from "react";
import { cn } from "@/libs/shadcn/utils";
import { ApiKeyService } from "@/services/api/apiKey";
import type { TokenSummary } from "@/types/apiToken";

interface MarkProps {
  /** The token no longer works: the dot is hollow. */
  off?: boolean;
  title?: string;
  children: React.ReactNode;
}

const Mark: React.FC<MarkProps> = ({ off, title, children }) => (
  <span className='inline-flex items-center gap-2' title={title}>
    <span
      aria-hidden='true'
      className={cn(
        "size-1.5 shrink-0 rounded-full",
        off ? "border border-muted-foreground" : "bg-foreground"
      )}
    />
    {children}
  </span>
);

/**
 * When a token dies, as a dot and a short phrase: "in 7 hours", "No expiry", "Expired",
 * "Revoked". The row is about the token, so its status is not the loudest thing in it.
 */
const TokenStatus: React.FC<{ token: TokenSummary }> = ({ token }) => {
  if (!token.is_active) return <Mark off>Revoked</Mark>;
  if (ApiKeyService.isExpired(token.expires_at)) return <Mark off>Expired</Mark>;

  const left = ApiKeyService.getTimeUntilExpiration(token.expires_at ?? undefined);
  if (left === null) {
    return (
      <Mark>
        <span className='sr-only'>Active, </span>No expiry
      </Mark>
    );
  }
  return (
    <Mark title={token.expires_at ? ApiKeyService.formatDate(token.expires_at) : undefined}>
      <span className='sr-only'>Active, expires </span>
      <span data-testid='api-key-expiry-countdown'>in {left}</span>
    </Mark>
  );
};

export default TokenStatus;
