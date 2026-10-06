export interface CliSession {
  /** The session JWT. Only the legacy handoff ever sends it anywhere. */
  token: string;
  email?: string;
}

/**
 * The browser's session, if it is *currently valid*, or `null`.
 *
 * A stale token (e.g. oxy restarted with a new signing key) would otherwise be handed over and
 * rejected, dead-ending the CLI with "token did not resolve to a user". `/api/user` returns the
 * user when valid and `null` when not. Throws on a network failure; callers treat that as no
 * session too.
 */
export const readCliSession = async (): Promise<CliSession | null> => {
  const token = localStorage.getItem("auth_token");
  if (!token) return null;
  const res = await fetch("/api/user", {
    headers: { Authorization: `Bearer ${token}` },
    credentials: "include"
  });
  const user: unknown = res.ok ? await res.json() : null;
  if (!user) return null;
  const email = typeof user === "object" ? (user as { email?: unknown }).email : undefined;
  return { token, email: typeof email === "string" ? email : undefined };
};

/** A full navigation: to the login page, or to oxyc's loopback listener. */
export const leaveTo = (url: string): void => {
  window.location.href = url;
};
