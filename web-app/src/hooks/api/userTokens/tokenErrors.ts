import { apiErrorCode, apiErrorMessage, apiStatus } from "@/libs/apiError";
import { exceedsPolicyMessage } from "@/libs/tokenPolicy";

/** What the person was doing when the request failed. */
export type TokenAction = "create" | "update" | "rename" | "regenerate" | "revoke";

/** The `code` a token route put in its `{ error, code? }` body, if any. */
export const tokenErrorCode = apiErrorCode;

const FALLBACK: Record<TokenAction, string> = {
  create: "Couldn't create the token",
  update: "Couldn't update the token's access",
  rename: "Couldn't rename the token",
  regenerate: "Couldn't regenerate the token",
  revoke: "Couldn't revoke the token"
};

/** A grant named an org or workspace the caller can't reach; the route answers 404, not 403. */
const UNREACHABLE =
  "One of the selected organizations or workspaces is no longer available to you. Reopen the dialog and pick again.";

/**
 * Toast copy for a failed `/user/tokens` request. The contract's codes come first, since two of
 * them share a status (403 `standing_required` / `session_required`).
 *
 * `legacy_immutable` is not handled: these routes serve personal access tokens only, and a legacy
 * API key's id answers 404 here, like any id that is not a token.
 */
export const tokenErrorMessage = (error: unknown, action: TokenAction): string => {
  const capped = exceedsPolicyMessage(error, action === "regenerate" ? "regenerate" : "set_expiry");
  if (capped) return capped;
  switch (tokenErrorCode(error)) {
    case "standing_required":
      return "Your account doesn't hold staff or partner access, so a token can't include it. Clear those options and try again.";
    case "revoked":
      return "This token was revoked, so it can't be changed.";
    case "session_required":
      return "Managing tokens needs a browser session. Sign in again, then retry.";
    case undefined:
      // No contract code: the status decides, below.
      break;
  }
  switch (apiStatus(error)) {
    case 400:
      return apiErrorMessage(error, FALLBACK[action]);
    case 404:
      return action === "create" || action === "update"
        ? UNREACHABLE
        : "This token no longer exists";
    default:
      return FALLBACK[action];
  }
};

/** The row on screen no longer matches the server: it was revoked or deleted elsewhere. */
export const isStaleTokenError = (error: unknown): boolean =>
  tokenErrorCode(error) === "revoked" || apiStatus(error) === 404;
