import { TokenExpiry } from "@/services/api/tokenExpiry";
import type { Token, TokenStatus } from "@/types/apiToken";

/**
 * A token's state as its row shows it. The server's `status` is as of the request. A token that
 * lapsed since is read as expired from its own `expires_at`, the way the Expiry cell reads it, so
 * a row never offers Revoke beside the word "Expired".
 */
export const tokenState = (token: Pick<Token, "status" | "expires_at">): TokenStatus => {
  // A legacy key is refused before anything else is judged, so its own expiry
  // never gets a say — including the four the migration leaves unrevoked.
  if (token.status === "removed") return "removed";
  if (token.status === "revoked") return "revoked";
  if (token.status === "expired" || TokenExpiry.isExpired(token.expires_at)) return "expired";
  return "active";
};

/** When a token stopped working: its revocation, or its expiry. `null` while it works. */
export const endedAt = (
  token: Pick<Token, "status" | "expires_at" | "revoked_at">
): string | null => {
  const state = tokenState(token);
  if (state === "revoked") return token.revoked_at;
  if (state === "expired") return token.expires_at;
  // A removed key has no moment of its own: it stopped when the release did.
  return null;
};

/** Days until a token lapses; `null` when it never does or has already ended. */
export const daysLeft = (
  token: Pick<Token, "status" | "expires_at">,
  now = new Date()
): number | null => {
  if (!token.expires_at || tokenState(token) !== "active") return null;
  const ms = new Date(token.expires_at).getTime() - now.getTime();
  return Math.floor(ms / (1000 * 60 * 60 * 24));
};

/**
 * The credential does not work and nothing can act on it: revoked, or a legacy
 * API key, whose format was removed on 2026-11-16.
 *
 * `removed` is the reason this exists. It is not `revoked` — the migration
 * leaves the keys that were still in use unrevoked so that reverting the image
 * can restore them — so every `status !== "revoked"` check would otherwise
 * offer Extend, Regenerate or Revoke on a key that authenticates nothing.
 */
export const tokenEnded = (token: Pick<Token, "status">): boolean =>
  token.status === "revoked" || token.status === "removed";
