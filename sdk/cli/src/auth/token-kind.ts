/**
 * What a credential string is, read off its prefix.
 *
 * Every kind goes out as `Authorization: Bearer` on `/api/**`; the kind only
 * decides the three things a prefix can decide without asking the server —
 * whether it may also ride `X-API-Key`, whether `DELETE /api/auth/token` can
 * end it, and whether it is a person or a machine. Pure on purpose: the
 * request layer imports this, and it must not pull the network in with it.
 */

export type CredentialShape =
  /** `oxy_pat_…` — a personal access token; what `oxyc login` stores. */
  | "personal"
  /** `oxy_sat_…` — a service account's long-lived token. */
  | "service_account"
  /** `oxy_ci_…` — fifteen minutes, minted from a GitHub OIDC token. */
  | "ci"
  /** `oxypublish_…` — the legacy app-scoped publish token. */
  | "publish"
  /** `oxy_<hex>` — a legacy API key. Not a token: `oxy_pat_` is never one of these. */
  | "legacy_key"
  /** Anything else: the session JWT an older `oxyc login` cached. */
  | "session";

export function credentialShape(token: string): CredentialShape {
  if (token.startsWith("oxy_pat_")) return "personal";
  if (token.startsWith("oxy_sat_")) return "service_account";
  if (token.startsWith("oxy_ci_")) return "ci";
  if (token.startsWith("oxypublish_")) return "publish";
  if (token.startsWith("oxy_")) return "legacy_key";
  return "session";
}

/**
 * Whether the string may be sent as `X-API-Key` on `/external/api/**`.
 *
 * A session JWT and a publish token are not API keys, and sending either
 * there would hand the server a key it must reject before it ever looks at
 * the bearer beside it.
 */
export function usableAsApiKey(token: string): boolean {
  const shape = credentialShape(token);
  return shape !== "session" && shape !== "publish";
}

/** Whether `DELETE /api/auth/token` can end it — new-format tokens only. */
export function isRevocable(token: string): boolean {
  const shape = credentialShape(token);
  return shape === "personal" || shape === "service_account" || shape === "ci";
}

/**
 * A service account's token, of either lifetime.
 *
 * Service accounts carry no platform standing, so a command with an admin
 * surface and a machine one (`oxyc checks run`) reads this to pick the one the
 * token can actually reach.
 */
export function isMachineIdentity(token: string): boolean {
  const shape = credentialShape(token);
  return shape === "service_account" || shape === "ci" || shape === "publish";
}
