import type {
  AuthConfigResponse,
  AuthResponse,
  DevLoginRequest,
  GitHubAuthRequest,
  GoogleAuthRequest,
  MagicLinkRequest,
  MagicLinkVerifyRequest,
  MessageResponse,
  OAuthStateResponse,
  OktaAuthRequest
} from "@/types/auth";
import { apiClient } from "./axios";

export class AuthService {
  /**
   * Mint a short-lived signed OAuth state token. Call before redirecting the
   * user to an external provider; echo the returned `state` through the
   * provider's `state` query param and back to the `/auth/{provider}` endpoint.
   */
  static async issueOAuthState(): Promise<OAuthStateResponse> {
    const response = await apiClient.post("/auth/oauth/state");
    return response.data;
  }

  static async googleAuth(request: GoogleAuthRequest): Promise<AuthResponse> {
    const response = await apiClient.post("/auth/google", request);
    return response.data;
  }

  static async oktaAuth(request: OktaAuthRequest): Promise<AuthResponse> {
    const response = await apiClient.post("/auth/okta", request);
    return response.data;
  }

  static async githubAuth(request: GitHubAuthRequest): Promise<AuthResponse> {
    const response = await apiClient.post("/auth/github", request);
    return response.data;
  }

  static async getAuthConfig(): Promise<AuthConfigResponse> {
    const response = await apiClient.get("/auth/config");
    return response.data;
  }

  /**
   * Hydrate auth state from the `oxy_session` cookie. Used on org-subdomain
   * boots where the cross-subdomain cookie is present but the per-origin
   * localStorage token is not. Resolves with a fresh token+user on 200;
   * rejects (401) when there's no valid session cookie.
   */
  static async getSession(): Promise<AuthResponse> {
    const response = await apiClient.get("/auth/session");
    return response.data;
  }

  static async requestMagicLink(request: MagicLinkRequest): Promise<MessageResponse> {
    const response = await apiClient.post("/auth/magic-link/request", request);
    return response.data;
  }

  static async verifyMagicLink(request: MagicLinkVerifyRequest): Promise<AuthResponse> {
    const response = await apiClient.post("/auth/magic-link/verify", request);
    return response.data;
  }

  /**
   * Dev-only sign-in bypass — mints the same session the real login flows do,
   * for an identity the backend pre-declared. The allow-list comes from
   * `OXY_DEV_LOGIN_EMAILS`, or on a **debug build** from `OXY_GLOBAL_ADMINS`,
   * in which case the server only answers loopback callers. 404s when neither
   * resolves, which is every release build that hasn't opted in — i.e. every
   * real deployment.
   */
  static async devLogin(request: DevLoginRequest = {}): Promise<AuthResponse> {
    const response = await apiClient.post("/auth/dev-login", request);
    return response.data;
  }

  /**
   * Trade a one-time browser ticket for a session — the `/token-login` path,
   * where an automation agent holding a personal API token is handed a URL
   * that signs a browser in. Answers with the same `AuthResponse` every login
   * returns and sets the `oxy_session` cookie itself. Public: the ticket is
   * the credential. Every refusal — unknown, already used, expired (tickets
   * work once and last 5 minutes), or the token behind it revoked — is one
   * `400 { code: "invalid_ticket" }`, deliberately indistinguishable.
   */
  static async redeemBrowserTicket(ticket: string): Promise<AuthResponse> {
    const response = await apiClient.post("/auth/browser-ticket/redeem", { ticket });
    return response.data;
  }

  /**
   * Ask the server whether `url` is safe to redirect a logged-in user to.
   * Returns true on 200, false on 403, false on any other response (treat
   * server errors as not-safe).
   */
  static async validateReturnTo(url: string): Promise<boolean> {
    try {
      const response = await apiClient.get("/auth/return-to/validate", {
        params: { url }
      });
      return response.status === 200;
    } catch {
      return false;
    }
  }

  /**
   * Tell the server to clear the `oxy_session` session cookie. That cookie is
   * HttpOnly, so the browser only drops it when the server replies with
   * `Set-Cookie: oxy_session=; Max-Age=0` — JS cannot delete it. Without this
   * round-trip the cookie survives logout and custom-app subdomains keep
   * loading with a still-valid session.
   *
   * Must be called while the bearer token is still in `localStorage`: this
   * route sits behind the auth middleware and the axios interceptor needs that
   * token to authenticate the request. Cloud-only route (404s in local mode),
   * so callers must tolerate failure.
   */
  static async logout(): Promise<void> {
    await apiClient.get("/logout");
  }
}
