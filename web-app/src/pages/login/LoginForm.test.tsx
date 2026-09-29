// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { BrowserRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthService, FrontlineService } from "@/services/api";
import type { BoundKioskDevice } from "@/types/frontline";
import LoginForm, { KioskLogin } from "./LoginForm";

/**
 * Where each sign-in provider sends the person signing in.
 *
 * On a kiosk the login URL's `return_to` is the crew's app — the enrol
 * redirect, crew sign-out and the custom-app login bounce all put it there —
 * so "Sign in as an admin" must land on `/kiosk` instead, for every provider.
 * The ordinary login page keeps following `return_to`.
 *
 * The seams are the service calls each provider makes with its destination:
 * the magic-link request carries it to the server, Google and Okta stash it
 * across the redirect, and GitHub validates it when its popup answers.
 */

// A Vite `define`; every build that is not the local OAuth bounce proxy sets it
// to "". Vitest defines nothing, and the OAuth hooks read it at import.
vi.hoisted(() => {
  (globalThis as Record<string, unknown>).__OXY_OAUTH_PROXY_ORIGIN__ = "";
});

vi.mock("@/services/api", () => ({
  AuthService: {
    issueOAuthState: vi.fn(),
    requestMagicLink: vi.fn(),
    validateReturnTo: vi.fn()
  },
  FrontlineService: { roster: vi.fn(), login: vi.fn() }
}));
vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({
    authConfig: {
      auth_enabled: true,
      mode: "cloud",
      magic_link: true,
      google: { client_id: "google-client" },
      okta: { client_id: "okta-client", domain: "acme.okta.com" },
      github: { client_id: "github-client" },
      dev_login: true,
      observability_enabled: false,
      billing_enabled: false
    },
    login: vi.fn()
  })
}));
// GitHub signs in through a popup; what the popup answers is the seam.
vi.mock("@/utils/githubAppInstall", () => ({ openSecureWindow: vi.fn(() => ({})) }));
vi.mock("@/utils/githubCallbackMessage", () => ({
  GitHubCallbackCancelled: class extends Error {},
  waitForGitHubCallback: vi.fn(async () => ({
    auth: {
      token: "jwt",
      user: { id: "u1", email: "maya@acme.test", name: "Maya", is_owner: false },
      orgs: []
    }
  }))
}));

const STASH_KEY = "oxy_post_login_return_to";
const APP_URL = "https://app.oxygen-hq.com/customer-apps/poke-house/store-ops/";
const kioskUrl = () => `${window.location.origin}/kiosk`;

const KIOSK: BoundKioskDevice = {
  bound: true,
  id: "kiosk-1",
  org: "poke-house",
  orgName: "Poke House",
  device: "Front counter",
  location: null,
  returnTo: APP_URL
};

const providers = (children: ReactNode) => (
  <QueryClientProvider client={new QueryClient()}>
    <BrowserRouter>{children}</BrowserRouter>
  </QueryClientProvider>
);

beforeEach(() => {
  // The URL a kiosk's login page is almost always on.
  window.history.replaceState(null, "", `/login?return_to=${encodeURIComponent(APP_URL)}`);
  vi.mocked(AuthService.issueOAuthState).mockResolvedValue({ state: "csrf" } as never);
  vi.mocked(AuthService.requestMagicLink).mockResolvedValue({ message: "sent" } as never);
  vi.mocked(AuthService.validateReturnTo).mockResolvedValue(false);
  vi.mocked(FrontlineService.roster).mockResolvedValue({ staff: [] });
});
afterEach(() => {
  cleanup();
  sessionStorage.clear();
  vi.clearAllMocks();
});

/** Render a login page and hand back where its account sign-in lives. */
async function signInOptions(kiosk: boolean) {
  const user = userEvent.setup();
  render(providers(kiosk ? <KioskLogin device={KIOSK} /> : <LoginForm />));
  if (!kiosk) return { user, scope: document.body };
  await user.click(screen.getByTestId("login-admin-signin"));
  return { user, scope: await screen.findByTestId("login-admin-dialog") };
}

async function destinations(kiosk: boolean) {
  const { user, scope } = await signInOptions(kiosk);
  const inScope = within(scope);

  await user.type(inScope.getByLabelText("Email"), "maya@acme.test");
  await user.click(inScope.getByRole("button", { name: "Continue with email" }));
  await waitFor(() => expect(AuthService.requestMagicLink).toHaveBeenCalled());
  const magicLink = vi.mocked(AuthService.requestMagicLink).mock.calls[0][0].return_to;

  await user.click(inScope.getByRole("button", { name: "Login with Google" }));
  await waitFor(() => expect(sessionStorage.getItem(STASH_KEY)).not.toBeNull());
  const google = sessionStorage.getItem(STASH_KEY);
  sessionStorage.clear();

  await user.click(inScope.getByRole("button", { name: "Login with Okta" }));
  await waitFor(() => expect(sessionStorage.getItem(STASH_KEY)).not.toBeNull());
  const okta = sessionStorage.getItem(STASH_KEY);

  await user.click(inScope.getByRole("button", { name: "Login with GitHub" }));
  await waitFor(() => expect(AuthService.validateReturnTo).toHaveBeenCalled());
  const github = vi.mocked(AuthService.validateReturnTo).mock.calls[0][0];

  const devLogin = inScope.getByTestId("login-dev-signin").getAttribute("href");
  return { magicLink, google, okta, github, devLogin };
}

describe("Sign in as an admin, on a kiosk", () => {
  it("lands every provider on /kiosk, not on the kiosk's app", async () => {
    const got = await destinations(true);
    expect(got).toEqual({
      magicLink: kioskUrl(),
      google: kioskUrl(),
      okta: kioskUrl(),
      github: kioskUrl(),
      devLogin: "/dev-login?next=%2Fkiosk"
    });
  });
});

describe("The ordinary login page", () => {
  it("still sends every provider to the login URL's return_to", async () => {
    const got = await destinations(false);
    expect(got).toEqual({
      magicLink: APP_URL,
      google: APP_URL,
      okta: APP_URL,
      github: APP_URL,
      devLogin: "/dev-login"
    });
  });
});
