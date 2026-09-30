// Sign a browser context in with an existing session token.
//
// A session for the PUBLIC port. The auth-disabled internal port (3001) is
// the easy target, but it carries neither `enforce_role` nor the ide proxy —
// so an IdeOnly route is served locally there instead of forwarded, and a
// replica answers it off a working copy it does not have. Driving the public
// port is the only way a browser test sees the routing a user sees, and that
// port needs a real session.
//
// `MAGIC_LINK_LOCAL_TEST=1` makes the backend write the sign-in email to a
// file instead of sending it, so the harness can mint one without a mailbox;
// dev-login (`GET /api/auth/dev-login?email=`) hands one back directly.
//
// BOTH halves are required. The backend reads the `oxy_session` cookie, but
// `AuthContext` decides whether the app is signed in by reading
// `localStorage.auth_token` — set the cookie alone and every route still
// redirects to /login, with the API perfectly willing to answer.

import type { BrowserContext } from "@playwright/test";

export async function signIn(
  context: BrowserContext,
  baseURL: string,
  token: string,
  user: string
): Promise<void> {
  const { hostname } = new URL(baseURL);
  await context.addCookies([
    {
      name: "oxy_session",
      value: token,
      domain: hostname,
      path: "/",
      httpOnly: true,
      sameSite: "Lax"
    }
  ]);
  await context.addInitScript(
    ([t, u]: [string, string]) => {
      localStorage.setItem("auth_token", t);
      if (u) localStorage.setItem("user", u);
    },
    [token, user] as [string, string]
  );
}
