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

export interface TokenSummaryLine {
  /** The answer to "is an agent running": set in the foreground. */
  lead: string;
  /** Who minted the live ones, and how many have ended. May be empty. */
  rest: string;
}

/**
 * The list in one sentence: how many tokens work now and for how many people, then how many no
 * longer do. The list keeps ended tokens, so "nothing is live" is not the same as "nothing listed".
 */
export const summarize = (
  tokens: Pick<Token, "status" | "expires_at" | "owner">[]
): TokenSummaryLine => {
  const live = tokens.filter((token) => tokenState(token) === "active");
  const ended = tokens.length - live.length;

  if (live.length === 0) {
    const below =
      ended === 1
        ? " The one below has expired or been revoked."
        : ` The ${ended} below have expired or been revoked.`;
    return { lead: "No agent holds a token right now.", rest: ended === 0 ? "" : below };
  }

  const minters = new Set(live.map((token) => token.owner.id)).size;
  const lead = live.length === 1 ? "1 token is live" : `${live.length} tokens are live`;
  const by = minters === 1 ? "one person" : `${minters} people`;
  const more =
    ended === 0 ? "" : ` ${ended} more ${ended === 1 ? "has" : "have"} expired or been revoked.`;
  return { lead, rest: `, minted by ${by}.${more}` };
};
