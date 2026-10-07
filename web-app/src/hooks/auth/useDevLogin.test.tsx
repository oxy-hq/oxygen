// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthService } from "@/services/api";
import type { AuthResponse } from "@/types/auth";
import { useDevLogin } from "./useDevLogin";

// `/dev-login` and `/token-login` share one destination rule
// (`resolvePostLoginDestination`, pinned in postLoginRedirect.test.ts). What
// stays dev-login's own is pinned here: it only ever *opens* a session, so it
// stores it without tearing anything down and lands with a soft navigation
// that replaces the `/dev-login` history entry.
const navigate = vi.fn();
const login = vi.fn();
vi.mock("react-router-dom", () => ({ useNavigate: () => navigate }));
vi.mock("@/contexts/AuthContext", () => ({ useAuth: () => ({ login }) }));
vi.mock("@/services/api", () => ({
  AuthService: { devLogin: vi.fn(), validateReturnTo: vi.fn() }
}));

const SIGNED_IN: AuthResponse = {
  token: "jwt",
  user: { id: "u1", email: "dev@oxy.local", name: "Dev", is_owner: false, is_app_admin: false },
  orgs: [{ id: "org-1", name: "Acme", slug: "acme", role: "owner" }]
};

const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient()}>{children}</QueryClientProvider>
);

beforeEach(() => {
  vi.mocked(AuthService.devLogin).mockResolvedValue(SIGNED_IN);
  vi.mocked(AuthService.validateReturnTo).mockResolvedValue(false);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  localStorage.clear();
  sessionStorage.clear();
});

describe("useDevLogin", () => {
  it("stores the session and soft-navigates to a same-origin next", async () => {
    localStorage.setItem("ide-branch-storage", "kept");
    const { result } = renderHook(() => useDevLogin({ next: "/ide" }), { wrapper });
    result.current.mutate({ as: "member" });

    await waitFor(() => expect(navigate).toHaveBeenCalledWith("/ide", { replace: true }));
    expect(login).toHaveBeenCalledWith(SIGNED_IN.token, SIGNED_IN.user);
    expect(localStorage.getItem("ide-branch-storage")).toBe("kept");
  });

  it("ignores an off-origin next and lands where the user's orgs say", async () => {
    const { result } = renderHook(() => useDevLogin({ next: "https://evil.example.com/" }), {
      wrapper
    });
    result.current.mutate({});

    await waitFor(() => expect(navigate).toHaveBeenCalledWith("/", { replace: true }));
  });

  it("reports a refusal through onFailure, and goes nowhere", async () => {
    const refused = new Error("Request failed with status code 403");
    vi.mocked(AuthService.devLogin).mockRejectedValue(refused);
    const onFailure = vi.fn();
    const { result } = renderHook(() => useDevLogin({ onFailure }), { wrapper });
    result.current.mutate({ email: "nope@nowhere.test" });

    await waitFor(() => expect(onFailure).toHaveBeenCalledWith(refused));
    expect(login).not.toHaveBeenCalled();
    expect(navigate).not.toHaveBeenCalled();
  });
});
