import { useMutation } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { useAuth } from "@/contexts/AuthContext";
import { authOrigin } from "@/libs/orgSubdomain";
import { AuthService } from "@/services/api";
import { openSecureWindow } from "@/utils/githubAppInstall";
import { waitForGitHubCallback } from "@/utils/githubCallbackMessage";
import { encodeOAuthState, OAUTH_PROXY_ORIGIN } from "./oauthProxyState";
import { handlePostLoginOrgs, resolveReturnTo, returnToFromUrl } from "./postLoginRedirect";

// The origin GitHub must redirect back to. Normally the registered auth origin;
// for local multi-instance dev the bounce proxy's origin is registered instead
// and forwards the callback here (see useGoogleAuth + oauthProxyState).
const GITHUB_AUTH_REDIRECT_URI = `${OAUTH_PROXY_ORIGIN || authOrigin()}/github/callback`;
const GITHUB_STATE_KEY = "github_oauth_login_state";

const buildGitHubAuthUrl = (clientId: string, state: string) => {
  const url = new URL("https://github.com/login/oauth/authorize");
  url.searchParams.set("client_id", clientId);
  url.searchParams.set("redirect_uri", GITHUB_AUTH_REDIRECT_URI);
  url.searchParams.set("scope", "read:user user:email");
  url.searchParams.set("state", state);
  return url.toString();
};

/**
 * Opens a popup to GitHub OAuth, waits for the unified /github/callback page
 * to postMessage the auth result, then signs the user in and redirects.
 * `destination` overrides where it lands — the kiosk's admin sign-in sends
 * `/kiosk`; absent, the login URL's own `return_to`.
 */
export const useGitHubAuth = (clientId: string, destination?: string) => {
  const { login } = useAuth();
  const navigate = useNavigate();

  return useMutation<void, Error, void>({
    mutationFn: async () => {
      // The whole flow runs in this (opener) window, so the login page's
      // `return_to` is still readable when the popup resolves — no stash needed.
      const returnTo = destination ?? returnToFromUrl();
      const { state } = await AuthService.issueOAuthState();
      // In proxy mode `state` carries this instance's origin so the bounce proxy
      // can forward the callback here; stored as-is so the CSRF check matches.
      const stateParam = encodeOAuthState(state);
      sessionStorage.setItem(GITHUB_STATE_KEY, stateParam);

      const popup = openSecureWindow(buildGitHubAuthUrl(clientId, stateParam));
      try {
        const result = await waitForGitHubCallback(popup, "auth");
        login(result.auth.token, result.auth.user);
        // Honor a `return_to` (e.g. a custom-app subdomain) before the default.
        const resolved = await resolveReturnTo(returnTo);
        if (resolved) {
          window.location.href = resolved;
          return;
        }
        navigate(handlePostLoginOrgs(result.auth.user, result.auth.orgs));
      } finally {
        sessionStorage.removeItem(GITHUB_STATE_KEY);
      }
    }
  });
};

export const validateGitHubAuthState = (receivedState: string | null): boolean => {
  if (!receivedState) return false;
  const storedState = sessionStorage.getItem(GITHUB_STATE_KEY);
  if (!storedState) return false;
  return receivedState === storedState;
};
