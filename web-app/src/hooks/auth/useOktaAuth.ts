import { useMutation } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { useAuth } from "@/contexts/AuthContext";
import { authOrigin } from "@/libs/orgSubdomain";
import ROUTES from "@/libs/utils/routes";
import { AuthService } from "@/services/api";
import type { AuthResponse, OktaAuthRequest } from "@/types/auth";
import {
  consumeReturnTo,
  handlePostLoginOrgs,
  resolveReturnTo,
  returnToFromUrl,
  stashReturnTo
} from "./postLoginRedirect";

// See useGoogleAuth: authOrigin() pins this to the registered app host on an
// org subdomain.
const OKTA_REDIRECT_URI = `${authOrigin()}/auth/okta/callback`;
const OKTA_STATE_KEY = "okta_oauth_state";

export const useOktaAuth = () => {
  const { login } = useAuth();
  const navigate = useNavigate();

  return useMutation<AuthResponse, Error, OktaAuthRequest>({
    mutationFn: AuthService.oktaAuth,
    onSuccess: async (data) => {
      // Clear state after successful authentication
      sessionStorage.removeItem(OKTA_STATE_KEY);
      login(data.token, data.user);
      // Honor a stashed `return_to` (e.g. a custom-app subdomain the user was
      // bounced from) before the default org/workspace destination.
      const returnTo = await resolveReturnTo(consumeReturnTo());
      if (returnTo) {
        window.location.href = returnTo;
        return;
      }
      const destination = handlePostLoginOrgs(data.user, data.orgs);
      navigate(destination);
    },
    onError: (error) => {
      console.error("Okta auth failed:", error);
      sessionStorage.removeItem(OKTA_STATE_KEY);
      navigate(ROUTES.AUTH.LOGIN);
    }
  });
};

/** Start Okta sign-in. `returnTo` as in `initiateGoogleAuth`. */
export const initiateOktaAuth = async (client_id: string, domain: string, returnTo?: string) => {
  // See useGoogleAuth.initiateGoogleAuth for CSRF design notes.
  const { state } = await AuthService.issueOAuthState();
  sessionStorage.setItem(OKTA_STATE_KEY, state);
  // Carry the destination across the provider round-trip so the callback can
  // send the user back where they came from.
  stashReturnTo(returnTo ?? returnToFromUrl());

  const url = new URL(`https://${domain}/oauth2/v1/authorize`);
  url.searchParams.set("client_id", client_id);
  url.searchParams.set("redirect_uri", OKTA_REDIRECT_URI);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("scope", "openid email profile");
  url.searchParams.set("state", state);

  window.location.href = url.toString();
};

/**
 * Validates the OAuth state parameter to prevent CSRF attacks
 * @param receivedState - The state parameter received in the callback
 * @returns true if state is valid, false otherwise
 */
export const validateOktaState = (receivedState: string | null): boolean => {
  if (!receivedState) {
    console.error("CSRF validation failed: No state parameter received");
    return false;
  }

  const storedState = sessionStorage.getItem(OKTA_STATE_KEY);
  if (!storedState) {
    console.error("CSRF validation failed: No stored state found");
    return false;
  }

  if (receivedState !== storedState) {
    console.error("CSRF validation failed: State mismatch");
    return false;
  }

  return true;
};
