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
  /**
   * `oxy_pat_…` — a personal access token; what `oxyc login` stores. An agent
   * token (`oxyc tokens create --agent`) is one too: the prefix cannot tell
   * them apart, only the token's own description can (`source: "oxyc_agent"`).
   */
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
  /**
   * `OXY_TOKEN=from-proxy` — no credential on this machine at all. The
   * environment's egress proxy adds `Authorization` to requests for the hosts
   * it lists (a Claude Code cloud environment's network secret), so the kind
   * is unknown here: only the server knows what the proxy attached.
   */
  | "proxy"
  /** Anything else: the session JWT an older `oxyc login` cached. */
  | "session";

/** What `oxyc tokens create --sandbox-agent` prints. */
export const SANDBOX_TOKEN_PREFIX = "oxy_sbx_";

/**
 * The value of `OXY_TOKEN` (or the `--token-env` variable) that says the
 * credential is added on the way out, by the egress proxy, and never held here.
 *
 * A SENTINEL IN THE VARIABLE, NOT A SEPARATE SWITCH, so it rides every path a
 * token already does: `--token-env`, `oxyc mcp`'s variable-only rule, a child
 * process's environment. Every header this CLI builds goes through
 * {@link bearerHeader}, which sends nothing for it — the proxy then adds the
 * header to a request that has none (`proxy-bearer.test.ts` holds every site
 * to that). A site that forgot would send `Bearer from-proxy`, which the
 * server turns away: it fails closed, never open.
 */
export const PROXY_CREDENTIAL = "from-proxy";

/** Whether the credential is the {@link PROXY_CREDENTIAL} sentinel. */
export function isProxyCredential(token: string | undefined): boolean {
  return token === PROXY_CREDENTIAL;
}

/**
 * `{ authorization: "Bearer <token>" }`, or nothing for the proxy sentinel.
 *
 * THE ONE PLACE A BEARER HEADER IS BUILT. Lower-case name: header names are
 * case-insensitive, and `fetch` sends them lower-cased anyway.
 */
export function bearerHeader(token: string | undefined): Record<string, string> {
  if (!token || isProxyCredential(token)) return {};
  return { authorization: `Bearer ${token}` };
}

export function credentialShape(token: string): CredentialShape {
  if (isProxyCredential(token)) return "proxy";
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

/** What `oxyc login` stores and `oxyc tokens create --agent` prints. */
export const PERSONAL_TOKEN_PREFIX = "oxy_pat_";

/** The same shape as a sandbox agent secret, under its own prefix. */
const PERSONAL_TOKEN_RE = /^oxy_pat_[0-9A-Za-z]{36}$/;

/**
 * Whether the string is a whole personal access token and nothing else.
 *
 * For the same sink as {@link isWellFormedSandboxToken}: `oxyc tokens create
 * --agent` writes its token into a line a shell will `eval`, so the prefix
 * alone is not enough to let a value through.
 */
export function isWellFormedPersonalToken(token: string | undefined): token is string {
  return typeof token === "string" && PERSONAL_TOKEN_RE.test(token);
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
  return (
    shape !== "session" &&
    shape !== "publish" &&
    shape !== "sandbox_agent" &&
    // Nothing here to send: the proxy adds `Authorization`, not `X-API-Key`.
    shape !== "proxy"
  );
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
