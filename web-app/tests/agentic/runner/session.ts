// Sign-in for the agentic runner, in two halves: which identity and workspace
// an enterprise-mode run drives (`ensureSession`, below), and how a browser
// context is signed in with a token once there is one (`signIn`, at the end).

import type { BrowserContext } from "@playwright/test";
import type { BackendMode } from "./types";

/**
 * The identity and workspace an enterprise-mode (`backend_mode: cloud`) run
 * drives by default.
 *
 * `flow@oxy.local` is an ordinary Owner of the `local` org with NO platform
 * standing — the same identity `scripts/seed-fixtures.sh` and
 * `scripts/fleet-assert.sh` sign the workspace flows in as. It must stay out of
 * the server's `OXY_GLOBAL_ADMINS` / `OXY_OWNER`: the SPA bounces a Global
 * Owner off every tenant workspace into /admin, and a flow signed in as staff
 * then times out on a testid that was never going to render.
 *
 * The workspace is the Demo workspace `oxy seed` writes at a deterministic id
 * (`Uuid::new_v5(NAMESPACE_DNS, "demo.oxy.local")`, crates/app/src/cli/commands/seed.rs).
 * The runner (backend.ts) and CI seed it from this checkout's `demo_project/`,
 * the fixture the flows are authored against.
 */
export const DEFAULT_FLOW_EMAIL = "flow@oxy.local";
export const DEMO_ORG_SLUG = "local";
export const DEMO_WORKSPACE_ID = "70787bb2-e11b-5488-b2c3-02e60d5fc7d3";

/** `OXY_FLOW_EMAIL` overrides the identity the runner signs in as. */
export function flowEmail(): string {
  return process.env.OXY_FLOW_EMAIL || DEFAULT_FLOW_EMAIL;
}

/** Workspace-scoped path prefix for the Demo workspace. */
export function demoPathPrefix(): string {
  return `/${DEMO_ORG_SLUG}/workspaces/${DEMO_WORKSPACE_ID}`;
}

export function isLoopbackUrl(url: string): boolean {
  const host = new URL(url).hostname;
  return host === "localhost" || host === "127.0.0.1" || host === "::1" || host === "[::1]";
}

/**
 * Give an enterprise-mode run a signed-in browser and a workspace to land in.
 *
 * Enterprise mode is the production path: the public port enforces auth, and
 * every workspace surface lives under `/<org>/workspaces/<id>`. So before any
 * flow runs the runner needs (a) a real session — minted through
 * `GET /api/auth/dev-login`, the same endpoint `/dev-login?as=` uses, which
 * answers only for identities in the server's `OXY_DEV_LOGIN_EMAILS` — and
 * (b) `OXY_PATH_PREFIX`, so a flow's `goto:/ide` resolves to the workspace
 * rather than the org picker.
 *
 * Both are defaults, never overrides: a caller that exported
 * `OXY_SESSION_TOKEN` (verify-all.sh's staff and fleet phases) or its own
 * `OXY_PATH_PREFIX` keeps what it set. `--no-auto-backend` does not skip this;
 * a pre-started backend needs the session just as much.
 */
export async function ensureSession(mode: BackendMode): Promise<void> {
  if (mode !== "cloud") return;

  if (!process.env.OXY_PATH_PREFIX) process.env.OXY_PATH_PREFIX = demoPathPrefix();

  if (process.env.OXY_SESSION_TOKEN) {
    console.log("[session] using the caller's OXY_SESSION_TOKEN");
    return;
  }

  const base = process.env.OXY_BASE_URL;
  if (!base) throw new Error("ensureSession: OXY_BASE_URL is unset");
  // Same rule as fixtures/reset.ts: nothing in this harness reaches past this
  // machine unless the caller says so in a variable named for what it does.
  if (!isLoopbackUrl(base) && process.env.OXY_FIXTURE_ALLOW_REMOTE !== "1") {
    throw new Error(
      `[session] refusing to mint a dev-login session on a non-loopback deployment (${base}). ` +
        "Export OXY_SESSION_TOKEN / OXY_SESSION_USER yourself, or set OXY_FIXTURE_ALLOW_REMOTE=1 " +
        "if that host is a disposable test deployment."
    );
  }

  const email = flowEmail();
  const { token, user } = await mintSession(base, email);
  process.env.OXY_SESSION_TOKEN = token;
  process.env.OXY_SESSION_USER = JSON.stringify(user ?? {});
  console.log(`[session] signed in as ${email}; workspace prefix ${process.env.OXY_PATH_PREFIX}`);
}

async function mintSession(base: string, email: string): Promise<{ token: string; user: unknown }> {
  const url = `${base.replace(/\/+$/, "")}/api/auth/dev-login?email=${encodeURIComponent(email)}`;
  const res = await fetch(url, { signal: AbortSignal.timeout(15_000) });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(
      `[session] dev-login as ${email} failed: ${res.status} — ${explain(res.status)}\n${body.slice(0, 300)}`
    );
  }
  const json = (await res.json()) as { token?: string; user?: unknown };
  if (!json.token) throw new Error(`[session] dev-login as ${email} answered without a token`);
  return { token: json.token, user: json.user };
}

/**
 * What each refusal from dev-login means for a test run. Every one of these
 * otherwise surfaces three minutes later as a locator timeout on /login.
 *
 * A 200 is not proof of a usable identity either: dev-login mints an account
 * for an allow-listed address it has never seen, and that account belongs to
 * no org, so every flow lands on /onboarding. Seeding first
 * (`OXY_GLOBAL_ADMINS=<flow email> oxy seed --workspace-path demo_project`)
 * is what makes it the Owner of `local`.
 */
export function explain(status: number): string {
  switch (status) {
    case 404:
      return "dev-login is disabled on this server: start it with OXY_DEV_LOGIN_EMAILS including the flow identity";
    case 403:
      return "the flow identity is not in the server's OXY_DEV_LOGIN_EMAILS";
    case 401:
      return "the flow identity's account is deleted; re-seed a fresh database";
    default:
      return "unexpected answer from /api/auth/dev-login";
  }
}

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
