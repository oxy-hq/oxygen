// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProtectedRoute from "@/components/ProtectedRoute";
import { AuthProvider } from "@/contexts/AuthContext";
import { FrontlineService } from "@/services/api";
import type { AuthConfigResponse } from "@/types/auth";
import type { BoundKioskDevice, KioskDevice } from "@/types/frontline";
import { KIOSK_SESSION_CHECK_MS } from "./useKioskSessionGuard";

// The kiosk probe is the only thing this check calls; everything between it
// and the page — ProtectedRoute, AuthProvider, the teardown `logout` shares —
// stays real.
vi.mock("@/services/api", () => ({
  AuthService: { validateReturnTo: vi.fn() },
  FrontlineService: { deviceStatus: vi.fn() }
}));
vi.mock("@/components/ui/shadcn/spinner", () => ({ Spinner: () => null }));

const deviceStatus = vi.mocked(FrontlineService.deviceStatus);

const AUTH_CONFIG = { auth_enabled: true, mode: "cloud" } as AuthConfigResponse;
const ADMIN_ID = "0b6d1c2e-admin";

const KIOSK: BoundKioskDevice = {
  bound: true,
  id: "kiosk-1",
  org: "acme",
  orgName: "Acme",
  device: "Santa Clara tablet",
  returnTo: "/customer-apps/acme/store-ops/"
};
const kiosk = (sessionUserId: string | null | undefined): KioskDevice =>
  sessionUserId === undefined ? KIOSK : { ...KIOSK, sessionUserId };

/** A token shaped like the server's: only the payload is ever read here. */
const tokenFor = (sub: string) =>
  `h.${btoa(JSON.stringify({ sub, exp: Math.floor(Date.now() / 1000) + 3600 }))}.s`;

// The token signed in with, compared by value: rebuilding it later reads the
// clock again, and a test that crosses a second boundary got a different `exp`.
let signedInToken = "";
const signInAsAdmin = () => {
  signedInToken = tokenFor(ADMIN_ID);
  localStorage.setItem("auth_token", signedInToken);
  localStorage.setItem("user", JSON.stringify({ id: ADMIN_ID, email: "maya@acme.test" }));
  sessionStorage.setItem("some-tab-state", "x");
};
const rememberKiosk = () => localStorage.setItem("oxy_kiosk_browser", "1");
// What the server sets beside the kiosk cookie, on every host that cookie
// covers — an org subdomain included, whose localStorage never saw a probe.
const writeCookie = (cookie: string) => {
  // biome-ignore lint/suspicious/noDocumentCookie: jsdom has no Cookie Store API; this stands in for the server's Set-Cookie
  document.cookie = cookie;
};
const setKioskHintCookie = () => writeCookie("oxy_kiosk_hint=1; Path=/");
const clearKioskHintCookie = () => writeCookie("oxy_kiosk_hint=; Max-Age=0; Path=/");

// Counts renders of the signed-in page, so "never shown" is checked, not
// just "gone by the end".
let adminPageRenders = 0;
function AdminPage() {
  adminPageRenders += 1;
  return <div data-testid='admin-page'>Leave kiosk mode</div>;
}

const renderKioskPage = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <AuthProvider authConfig={AUTH_CONFIG}>
        <MemoryRouter initialEntries={["/kiosk"]}>
          <Routes>
            <Route path='/login' element={<div data-testid='login-page'>name board</div>} />
            <Route
              path='/kiosk'
              element={
                <ProtectedRoute>
                  <AdminPage />
                </ProtectedRoute>
              }
            />
          </Routes>
        </MemoryRouter>
      </AuthProvider>
    </QueryClientProvider>
  );
};

let navigatedTo: string | null;
const realLocation = window.location;

beforeEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  clearKioskHintCookie();
  adminPageRenders = 0;
  navigatedTo = null;
  // jsdom cannot navigate; record where the page was sent instead.
  Object.defineProperty(window, "location", {
    configurable: true,
    value: {
      origin: "http://127.0.0.1:5173",
      pathname: "/kiosk",
      get href() {
        return navigatedTo ?? "http://127.0.0.1:5173/kiosk";
      },
      set href(url: string) {
        navigatedTo = url;
      }
    }
  });
  Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.useRealTimers();
  Object.defineProperty(window, "location", { configurable: true, value: realLocation });
});

const expectSignedOut = async () => {
  await waitFor(() => expect(navigatedTo).toBe("/login"));
  expect(localStorage.getItem("auth_token")).toBeNull();
  expect(localStorage.getItem("user")).toBeNull();
  expect(sessionStorage.getItem("some-tab-state")).toBeNull();
};

const expectStillSignedIn = () => {
  expect(navigatedTo).toBeNull();
  expect(localStorage.getItem("auth_token")).toBe(signedInToken);
  expect(screen.getByTestId("admin-page")).toBeTruthy();
};

describe("on a kiosk holding an admin's stored sign-in", () => {
  it("clears it and goes to /login when the session cookie is gone, never showing the page", async () => {
    rememberKiosk();
    signInAsAdmin();
    deviceStatus.mockResolvedValue(kiosk(null));

    renderKioskPage();

    await expectSignedOut();
    expect(adminPageRenders).toBe(0);
    // The device memory is not per-user; signing out leaves it.
    expect(localStorage.getItem("oxy_kiosk_browser")).toBe("1");
  });

  it("clears it when the cookie now carries someone else's session", async () => {
    rememberKiosk();
    signInAsAdmin();
    deviceStatus.mockResolvedValue(kiosk("crew-worker-id"));

    renderKioskPage();

    await expectSignedOut();
    expect(adminPageRenders).toBe(0);
  });

  it("keeps it while the cookie still carries the same session", async () => {
    rememberKiosk();
    signInAsAdmin();
    deviceStatus.mockResolvedValue(kiosk(ADMIN_ID));

    renderKioskPage();

    await screen.findByTestId("admin-page");
    expect(deviceStatus).toHaveBeenCalledTimes(1);
    expectStillSignedIn();
  });

  it("keeps it when the probe fails — unknown is never signed out", async () => {
    rememberKiosk();
    signInAsAdmin();
    deviceStatus.mockRejectedValue(new Error("Network Error"));

    renderKioskPage();

    await screen.findByTestId("admin-page");
    expect(deviceStatus).toHaveBeenCalledTimes(1);
    expectStillSignedIn();
  });

  it("keeps it when the server is too old to report the cookie", async () => {
    rememberKiosk();
    signInAsAdmin();
    deviceStatus.mockResolvedValue(kiosk(undefined));

    renderKioskPage();

    await screen.findByTestId("admin-page");
    expectStillSignedIn();
  });
});

describe("on an origin whose localStorage was never told it is a kiosk", () => {
  it("still checks when the kiosk hint cookie is set, and signs out a sign-in the cookie no longer backs", async () => {
    // An org subdomain: the HQ tab rehydrated the manager's session into this
    // origin's storage, and nothing here ever ran the kiosk probe.
    setKioskHintCookie();
    signInAsAdmin();
    deviceStatus.mockResolvedValue(kiosk(null));

    renderKioskPage();

    await expectSignedOut();
    expect(deviceStatus).toHaveBeenCalledTimes(1);
    expect(adminPageRenders).toBe(0);
  });

  it("keeps a sign-in the cookie still backs", async () => {
    setKioskHintCookie();
    signInAsAdmin();
    deviceStatus.mockResolvedValue(kiosk(ADMIN_ID));

    renderKioskPage();

    await screen.findByTestId("admin-page");
    expectStillSignedIn();
  });
});

describe("the kiosk memory survives the check's own answers", () => {
  it("is not forgotten when a check reads 'not a kiosk'", async () => {
    rememberKiosk();
    signInAsAdmin();
    deviceStatus.mockResolvedValue({ bound: false });

    renderKioskPage();

    await screen.findByTestId("admin-page");
    expect(deviceStatus).toHaveBeenCalledTimes(1);
    expectStillSignedIn();
    // Only leaving, revoking or binding changes it; a poll never does, so one
    // bad answer cannot switch the check off for good.
    expect(localStorage.getItem("oxy_kiosk_browser")).toBe("1");
  });
});

describe("re-checking while the page stays open", () => {
  const openSignedIn = async () => {
    rememberKiosk();
    signInAsAdmin();
    deviceStatus.mockResolvedValue(kiosk(ADMIN_ID));
    renderKioskPage();
    await screen.findByTestId("admin-page");
    expect(deviceStatus).toHaveBeenCalledTimes(1);
    // Somewhere else on the tablet, a custom app's idle timer signs out.
    deviceStatus.mockResolvedValue(kiosk(null));
  };

  it("re-checks when the tab becomes visible", async () => {
    await openSignedIn();
    act(() => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await expectSignedOut();
    expect(deviceStatus).toHaveBeenCalledTimes(2);
  });

  it("re-checks when the window regains focus", async () => {
    await openSignedIn();
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    await expectSignedOut();
  });

  it("re-checks every minute", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    await openSignedIn();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(KIOSK_SESSION_CHECK_MS);
    });
    await expectSignedOut();
  });
});

describe("where it stays silent", () => {
  it("makes no call on a browser that was never a kiosk", async () => {
    signInAsAdmin();

    renderKioskPage();

    await screen.findByTestId("admin-page");
    expect(deviceStatus).not.toHaveBeenCalled();
    expectStillSignedIn();
  });

  it("makes no call on a kiosk with no stored sign-in, as a crew PIN session has none", async () => {
    rememberKiosk();

    renderKioskPage();

    await screen.findByTestId("login-page");
    expect(deviceStatus).not.toHaveBeenCalled();
    expect(navigatedTo).toBeNull();
  });
});
