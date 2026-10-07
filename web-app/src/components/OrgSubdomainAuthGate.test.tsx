// @vitest-environment jsdom

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthProvider } from "@/contexts/AuthContext";
import { redirectToCentralLogin } from "@/libs/orgSubdomain";
import { AuthService } from "@/services/api";
import type { AuthConfigResponse } from "@/types/auth";
import OrgSubdomainAuthGate from "./OrgSubdomainAuthGate";

/**
 * A signed-out browser on an org subdomain is bounced to the app-host login —
 * except on a sign-in link, whose fragment holds a one-time ticket.
 *
 * The bounce carries the whole current URL as `return_to`, so following it
 * from `/token-login#ticket=…` would turn a fragment (never sent) into a query
 * string (sent, and logged). The seams are the two things jsdom cannot do: the
 * cookie-hydration call, and the cross-origin navigation the bounce is.
 */
vi.mock("@/services/api", () => ({ AuthService: { getSession: vi.fn() } }));
vi.mock("@/libs/orgSubdomain", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/libs/orgSubdomain")>()),
  redirectToCentralLogin: vi.fn(() => true)
}));

const getSession = vi.mocked(AuthService.getSession);
const bounce = vi.mocked(redirectToCentralLogin);
const AUTH_CONFIG = { auth_enabled: true, mode: "cloud" } as AuthConfigResponse;

function renderGateAt(url: string) {
  window.history.replaceState(null, "", url);
  render(
    <AuthProvider authConfig={AUTH_CONFIG}>
      <OrgSubdomainAuthGate>
        <div data-testid='app'>app</div>
      </OrgSubdomainAuthGate>
    </AuthProvider>
  );
}

beforeEach(() => {
  localStorage.clear();
  window.__OXY_ORG__ = {
    orgId: "org-1",
    orgSlug: "acme",
    subdomain: "acme",
    appBaseUrl: "https://app.example.com"
  };
  // No session cookie: the hydration every case here starts from.
  getSession.mockRejectedValue({ response: { status: 401 } });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  delete window.__OXY_ORG__;
  window.history.replaceState(null, "", "/");
});

describe("OrgSubdomainAuthGate, signed out on an org subdomain", () => {
  it("lets a sign-in link through to redeem itself, ticket still in the fragment", async () => {
    renderGateAt("/token-login#ticket=abc&next=%2Fide");

    expect(await screen.findByTestId("app")).toBeTruthy();
    expect(bounce).not.toHaveBeenCalled();
    expect(window.location.hash).toBe("#ticket=abc&next=%2Fide");
  });

  it("bounces every other page to the app-host login", async () => {
    // The control: the same signed-out browser, one path over.
    renderGateAt("/ide#ticket=abc");

    await waitFor(() => expect(bounce).toHaveBeenCalledTimes(1));
    expect(screen.queryByTestId("app")).toBeNull();
  });
});
