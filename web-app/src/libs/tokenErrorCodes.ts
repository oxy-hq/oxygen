/**
 * Every `code` a token route puts beside `error` (tokens HTTP contract). A response carrying one
 * has already said exactly why it refused, and the surface that made the call words it for the
 * person, so the API client's generic denial toast stays quiet for these.
 */
const TOKEN_ERROR_CODES = [
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
  // An organization's service-account token asked for through oxyc: the approval page says
  // each of these in its own words.
  "org_admin_required",
  "org_not_found",
  "role_mismatch",
  "service_account_disabled",
  "invalid_service_account_token",
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

const APP_ENVIRONMENT_PATH = /^\/?customer-apps\/[^/]+\/(environments|secrets)(\/|\?|$)/;

/**
 * A custom app's environment and secrets routes. Their 403 means "may not open this app's
 * non-production environments" (oxy-authz `AppNonProduction`), and the admin Secrets pane says
 * so inline beside the environment picker — so the generic denial toast would be a second
 * message, and one that fires every time the pane opens for a viewer without that ring.
 */
export const isAppEnvironmentPath = (url: string): boolean => APP_ENVIRONMENT_PATH.test(url);
