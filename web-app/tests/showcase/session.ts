// A signed-in session for the showcase identity, minted through dev-login's GET
// form (the token in the body, no cookie). The runner sets the cookie and the
// SPA's localStorage from it, so the recording starts already signed in.

import { SHOWCASE_USER } from "./types";

export interface Session {
  token: string;
  user: string;
}

export async function mintSession(backendUrl: string): Promise<Session> {
  const email = SHOWCASE_USER;
  const url = `${backendUrl}/api/auth/dev-login?email=${encodeURIComponent(email)}`;
  const res = await fetch(url);
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(
      `dev-login as ${email} answered ${res.status}: ${body.slice(0, 200)} — ` +
        "is the address in OXY_DEV_LOGIN_EMAILS, and did the seed create it?"
    );
  }
  const body = (await res.json()) as { token?: string; user?: unknown };
  if (!body.token) throw new Error(`dev-login as ${email} returned no token`);
  return { token: body.token, user: JSON.stringify(body.user ?? {}) };
}
