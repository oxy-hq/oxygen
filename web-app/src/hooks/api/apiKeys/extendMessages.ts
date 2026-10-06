import { apiErrorCode, apiErrorMessage, apiStatus } from "@/libs/apiError";
import { exceedsPolicyMessage } from "@/libs/tokenPolicy";
import { ApiKeyService } from "@/services/api/apiKey";
import type { TokenKind } from "@/types/apiToken";

/**
 * What to call the thing in copy. The two are never swapped: a legacy API key is never called a
 * token, and a token is never called a key.
 */
export type TokenNoun = "legacy API key" | "token";

/**
 * The noun for one row, from its `kind`. A row with no `kind` came from the legacy
 * `/api-keys` routes, which list legacy API keys only.
 */
export const tokenNoun = (token: { kind?: TokenKind }): TokenNoun =>
  token.kind && token.kind !== "legacy_key" ? "token" : "legacy API key";

/** Toast copy for a failed extend, by the contract's status codes. */
export const extendErrorMessage = (error: unknown, noun: TokenNoun = "legacy API key"): string => {
  const capped = exceedsPolicyMessage(error);
  if (capped) return capped;
  // A 409 too, and not a revocation: a sandbox agent token's lifetime is fixed when it is minted.
  if (apiErrorCode(error) === "sandbox_token_fixed") {
    return "A sandbox agent token can't be extended. Create a new one when it lapses.";
  }
  switch (apiStatus(error)) {
    case 409:
      return `This ${noun} was revoked and can't be extended`;
    case 400:
      return apiErrorMessage(error, "That expiry isn't valid");
    case 403:
      return "Extending needs a browser session";
    case 404:
      return `This ${noun} no longer exists`;
    default:
      return `Couldn't extend the ${noun}`;
  }
};

/** 404 (deleted) or 409 (revoked): the row on screen no longer matches the server. */
export const isStaleKeyError = (error: unknown): boolean => {
  const status = apiStatus(error);
  return status === 404 || status === 409;
};

/** Toast copy for a successful extend, by name. One that had expired and comes back says so. */
export const extendSuccessMessage = (
  before: { expires_at?: string | null },
  after: { name: string; expires_at?: string | null }
): string => {
  const revived = ApiKeyService.isExpired(before.expires_at);
  if (!after.expires_at) {
    return revived
      ? `"${after.name}" is active again and no longer expires`
      : `"${after.name}" no longer expires`;
  }
  const until = ApiKeyService.formatDay(after.expires_at);
  return revived
    ? `"${after.name}" is active again until ${until}`
    : `"${after.name}" now expires ${until}`;
};
