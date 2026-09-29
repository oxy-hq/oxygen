/**
 * Centralized teardown for everything tied to the signed-in user.
 *
 * Called by `AuthContext.logout()` and the axios 401 interceptor so that a
 * different user signing in on the same browser does not inherit the prior
 * user's IDE branch selections, open database tabs, or stale sync state.
 *
 * The theme preference is intentionally kept — it's a device-level setting,
 * not a per-user one, and clearing it makes the page flash on sign-in.
 */

const PERSISTED_AUTH_SCOPED_KEYS = [
  "auth_token",
  "user",
  "ide-branch-storage",
  "database-client-storage",
  "database-operation-storage"
] as const;

export function clearAuthScopedStorage(): void {
  for (const key of PERSISTED_AUTH_SCOPED_KEYS) {
    try {
      localStorage.removeItem(key);
    } catch {
      // localStorage may be disabled (private browsing, quota); failing
      // silently is acceptable — the next session will simply rehydrate
      // whatever survived.
    }
  }
}

/**
 * True when there is no stored auth token, or the stored JWT has expired (or
 * can't be parsed). `isAuthenticated()` only checks token *presence*, so an
 * expired-but-present token would skip cookie hydration on an org subdomain and
 * then 401 on the first API call — this lets the hydration gate also fire on a
 * stale token. Pure client-side `exp` read; the server still re-validates.
 */
export function isAuthTokenExpired(): boolean {
  const claims = readStoredTokenClaims();
  return !claims || typeof claims.exp !== "number" || claims.exp * 1000 <= Date.now();
}

/**
 * The user id the stored auth token was minted for (its `sub`), or null when
 * there is no token or it can't be parsed. Pure client-side read, like
 * {@link isAuthTokenExpired}; the server still validates the token itself.
 */
export function storedTokenSubject(): string | null {
  const sub = readStoredTokenClaims()?.sub;
  return typeof sub === "string" && sub !== "" ? sub : null;
}

function readStoredTokenClaims(): { exp?: unknown; sub?: unknown } | null {
  let token: string | null = null;
  try {
    token = localStorage.getItem("auth_token");
  } catch {
    return null;
  }
  if (!token) return null;
  try {
    const payload = token.split(".")[1];
    const claims: unknown = JSON.parse(atob(payload.replace(/-/g, "+").replace(/_/g, "/")));
    return claims && typeof claims === "object" ? claims : null;
  } catch {
    return null;
  }
}
