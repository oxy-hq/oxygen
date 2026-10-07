import { ApiKeyService } from "@/services/api/apiKey";
import type { Token, TokenStatus } from "@/types/apiToken";

/**
 * A token's state as its row shows it. The server's `status` is as of the request. A token that
 * lapsed since is read as expired from its own `expires_at`, the way the Expiry cell reads it, so
 * a row never offers Revoke beside the word "Expired".
 */
export const tokenState = (token: Pick<Token, "status" | "expires_at">): TokenStatus => {
  if (token.status === "revoked") return "revoked";
  if (token.status === "expired" || ApiKeyService.isExpired(token.expires_at)) return "expired";
  return "active";
};

/** When a token stopped working: its revocation, or its expiry. `null` while it works. */
export const endedAt = (
  token: Pick<Token, "status" | "expires_at" | "revoked_at">
): string | null => {
  const state = tokenState(token);
  if (state === "revoked") return token.revoked_at;
  if (state === "expired") return token.expires_at;
  return null;
};
