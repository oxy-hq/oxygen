import { isAxiosError } from "axios";
import { apiErrorCode } from "@/libs/apiError";
import { exceedsPolicyMessage } from "@/libs/tokenPolicy";

export { apiErrorCode };

/** Each says what happened and what to do next, in the interface's voice. */
const CODE_MESSAGES: Record<string, string> = {
  session_required:
    "This can only be done from a signed-in browser, not with a token. Sign in again and retry.",
  name_taken:
    "A service account with that name already exists in this organization. Pick another name.",
  repository_unresolved:
    "We couldn't look up that repository's ids, which happens for a private repository this organization hasn't connected. Enter them below.",
  environment_required:
    "This organization requires an environment on every trusted-access policy. Name one, or relax the rule under Policy.",
  legacy_immutable: "Legacy API keys can only be extended or revoked by their owner.",
  use_service_account_routes:
    "This token belongs to a service account. Revoke it from the account's own page.",
  revoked: "This token has been revoked, so it can't be extended. Create a new one instead.",
  standing_required: "That needs platform or partner standing this account doesn't hold."
};

/** Statuses whose server sentence ("Not found") says less than we can. */
const STATUS_MESSAGES: Record<number, string> = {
  401: "Your session has ended. Sign in again and retry.",
  403: "You need to be an organization owner or admin to do this.",
  404: "That no longer exists. It may have been deleted, or this server doesn't support it yet."
};

function serverSentence(err: unknown): string | undefined {
  if (!isAxiosError(err)) return undefined;
  const data: unknown = err.response?.data;
  if (!data || typeof data !== "object") return undefined;
  const body = data as { error?: unknown; message?: unknown };
  if (typeof body.error === "string" && body.error) return body.error;
  if (typeof body.message === "string" && body.message) return body.message;
  return undefined;
}

/**
 * One sentence for a failed request, most specific source first: a code we
 * know, then a status that speaks for itself, then the server's own words,
 * then the caller's fallback. A request that never got an answer says so.
 */
export function describeApiError(err: unknown, fallback: string): string {
  const capped = exceedsPolicyMessage(err);
  if (capped) return capped;
  const code = apiErrorCode(err);
  if (code && CODE_MESSAGES[code]) return CODE_MESSAGES[code];

  if (isAxiosError(err)) {
    if (!err.response) return "Couldn't reach the server. Check your connection and try again.";
    const byStatus = STATUS_MESSAGES[err.response.status];
    if (byStatus) return byStatus;
  }
  return serverSentence(err) ?? fallback;
}

/** Whether a failed request carries exactly this contract code. */
export const isApiErrorCode = (err: unknown, code: string): boolean => apiErrorCode(err) === code;
