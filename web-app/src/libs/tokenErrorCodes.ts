/**
 * Every `code` a token route puts beside `error` (tokens HTTP contract). A response carrying one
 * has already said exactly why it refused, and the surface that made the call words it for the
 * person, so the API client's generic denial toast stays quiet for these.
 */
export const TOKEN_ERROR_CODES = [
  // Personal tokens and the calling-token routes.
  "session_required",
  "standing_required",
  "legacy_immutable",
  "revoked",
  "no_token",
  "invalid_code",
  // Organization → API access.
  "name_taken",
  "use_service_account_routes",
  "repository_unresolved",
  "environment_required",
  // Policy and hygiene.
  "exceeds_policy",
  "rate_limited"
] as const;

export type TokenErrorCode = (typeof TOKEN_ERROR_CODES)[number];

export const isTokenErrorCode = (code: unknown): code is TokenErrorCode =>
  typeof code === "string" && (TOKEN_ERROR_CODES as readonly string[]).includes(code);

const ORG_API_ACCESS_PATH = /^\/?orgs\/[^/]+\/(service-accounts|tokens|token-policy)(\/|\?|$)/;

/**
 * Organization → API access routes. Their bare 403 (no `code`) means "not an org owner or admin",
 * and the section already says so — a forbidden notice on a list, its own sentence on an action —
 * so the API client's generic denial toast would be a second message for the same refusal.
 */
export const isOrgApiAccessPath = (url: string): boolean => ORG_API_ACCESS_PATH.test(url);
