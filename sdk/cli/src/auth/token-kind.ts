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
  /**
   * `oxy_sbx_…` — a sandbox agent token: the sandbox loop on one to five named
   * apps, hours long, and nothing else on the deployment.
   */
  | "sandbox_agent"
  /** `oxypublish_…` — the legacy app-scoped publish token. */
  | "publish"
  /** `oxy_<hex>` — a legacy API key. Not a token: `oxy_pat_` is never one of these. */
  | "legacy_key"
  /** Anything else: the session JWT an older `oxyc login` cached. */
  | "session";

/** What `oxyc tokens create --sandbox-agent` prints. */
export const SANDBOX_TOKEN_PREFIX = "oxy_sbx_";

export function credentialShape(token: string): CredentialShape {
  if (token.startsWith("oxy_pat_")) return "personal";
  if (token.startsWith("oxy_sat_")) return "service_account";
  if (token.startsWith("oxy_ci_")) return "ci";
  // Before the `oxy_` fall-through, or it reads as a legacy API key.
  if (token.startsWith(SANDBOX_TOKEN_PREFIX)) return "sandbox_agent";
  if (token.startsWith("oxypublish_")) return "publish";
  if (token.startsWith("oxy_")) return "legacy_key";
  return "session";
}

/** Whether the credential is a sandbox agent token, read off the prefix alone. */
export function isSandboxAgentToken(token: string | undefined): token is string {
  return token?.startsWith(SANDBOX_TOKEN_PREFIX) === true;
}

/** The prefix, thirty base62 characters and six of checksum: a whole secret. */
const SANDBOX_TOKEN_RE = /^oxy_sbx_[0-9A-Za-z]{36}$/;

/**
 * Whether the string is a whole sandbox agent secret and nothing else.
 *
 * {@link isSandboxAgentToken} reads the prefix, which is enough to choose a
 * code path. It is NOT enough before the value is written into a line a shell
 * will `eval` (`export OXY_TOKEN=…`): a deployment that answered
 * `oxy_sbx_x; <command>` would have that command run on the operator's
 * machine. Letters and digits only leave nothing for a shell to interpret.
 */
export function isWellFormedSandboxToken(token: string | undefined): token is string {
  return typeof token === "string" && SANDBOX_TOKEN_RE.test(token);
}

/**
 * Whether the string may be sent as a bearer at all: the characters every
 * credential this CLI handles is made of (a token, a legacy key, a JWT), and a
 * bounded length. Anything else a deployment hands back is not a credential.
 */
export function isHeaderSafeSecret(secret: string | undefined): secret is string {
  return typeof secret === "string" && /^[A-Za-z0-9._-]{8,2048}$/.test(secret);
}

/**
 * Whether the string may be sent as `X-API-Key` on `/external/api/**`.
 *
 * A session JWT and a publish token are not API keys, and sending either
 * there would hand the server a key it must reject before it ever looks at
 * the bearer beside it. A sandbox agent token is refused on that whole surface.
 */
export function usableAsApiKey(token: string): boolean {
  const shape = credentialShape(token);
  return shape !== "session" && shape !== "publish" && shape !== "sandbox_agent";
}

/** Whether `DELETE /api/auth/token` can end it — new-format tokens only. */
export function isRevocable(token: string): boolean {
  const shape = credentialShape(token);
  return (
    shape === "personal" ||
    shape === "service_account" ||
    shape === "ci" ||
    shape === "sandbox_agent"
  );
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
