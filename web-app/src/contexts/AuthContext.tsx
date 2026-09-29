import type React from "react";
import { createContext, useCallback, useContext, useEffect } from "react";
import { redirectToHome } from "@/libs/utils";
import { clearAuthScopedStorage } from "@/libs/utils/authStorage";
import {
  clearAllOnboardingState,
  clearLegacyLocalOnboardingState
} from "@/libs/utils/onboardingStorage";
import { AuthService } from "@/services/api/auth";
import type { AuthConfigResponse, UserInfo } from "@/types/auth";

interface AuthContextType {
  getUser: () => string | null;
  getToken: () => string | null;
  isAuthenticated: () => boolean;
  login: (token: string, user: UserInfo) => void;
  // Async: awaits the backend `/logout` (clears the HttpOnly `oxy_session`
  // cookie) before tearing down local state. Callers may await the teardown.
  logout: () => Promise<void>;
  authConfig: AuthConfigResponse;
  isLocalMode: boolean;
}

const AuthContext = createContext<AuthContextType | undefined>(undefined);

/**
 * Everything `logout` clears in this browser, without the server round-trip or
 * the redirect. Also what a kiosk runs when its session cookie is found gone
 * (`useKioskSessionGuard`): one teardown, so the two cannot drift.
 */
export function clearLocalSession(): void {
  clearAuthScopedStorage();
  // Clear any in-flight wizard state so a different user signing in on the
  // same browser doesn't inherit it (and so the same user can't be re-trapped
  // in pending onboarding for a workspace they intentionally walked away from).
  clearAllOnboardingState();
  sessionStorage.clear();
}

export const useAuth = () => {
  const context = useContext(AuthContext);
  if (!context) {
    throw new Error("useAuth must be used within an AuthProvider");
  }
  return context;
};

interface AuthProviderProps {
  children: React.ReactNode;
  authConfig: AuthConfigResponse;
}

export const AuthProvider: React.FC<AuthProviderProps> = ({ children, authConfig }) => {
  const login = useCallback((newToken: string, newUser: UserInfo) => {
    localStorage.setItem("auth_token", newToken);
    localStorage.setItem("user", JSON.stringify(newUser));
  }, []);

  const logout = async () => {
    // Clear the server-set `oxy_session` cookie FIRST, while the bearer token
    // is still in localStorage so the auth-gated `/logout` request
    // authenticates. The cookie is HttpOnly — JS cannot delete it — so without
    // this round-trip it survives logout and custom-app subdomains keep
    // loading with a still-valid session. Tolerate any failure (network,
    // expired token, local-mode 404) so we never trap the user in a
    // half-logged-out state.
    try {
      await AuthService.logout();
    } catch (error) {
      console.error("Backend logout failed; clearing local session anyway.", error);
    }
    clearLocalSession();
    redirectToHome();
  };

  const storedUser = () => localStorage.getItem("user");
  const storedToken = () => localStorage.getItem("auth_token");

  const isLocalMode = authConfig.mode === "local";

  // The nil-UUID key shape can only have come from the legacy local-mode
  // path, so running this unconditionally (rather than gating on
  // `isLocalMode`) also cleans up after a dev who alternated `--local`
  // and cloud on the same origin.
  useEffect(() => {
    clearLegacyLocalOnboardingState();
  }, []);

  const value: AuthContextType = {
    getUser: storedUser,
    getToken: storedToken,
    // In local mode the backend auto-authenticates a guest user, so the UI
    // should treat every session as authenticated regardless of localStorage.
    isAuthenticated: () => isLocalMode || (!!storedToken() && !!storedUser()),
    login,
    logout,
    authConfig,
    isLocalMode
  };

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
};
