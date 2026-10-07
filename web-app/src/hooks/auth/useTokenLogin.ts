import { useMutation } from "@tanstack/react-query";
import { clearLocalSession, useAuth } from "@/contexts/AuthContext";
import { AuthService } from "@/services/api";
import type { AuthResponse } from "@/types/auth";
import {
  leaveTo,
  type RequestedDestination,
  resolvePostLoginDestination
} from "./postLoginRedirect";

interface TokenLoginOptions extends RequestedDestination {
  /**
   * Called when the redeem fails. Hook-level for the reason `useDevLogin`
   * gives: `/token-login` fires from an effect whose StrictMode pass is
   * discarded, and React Query skips per-call callbacks for an observer that
   * is no longer subscribed.
   */
  onFailure?: (error: Error) => void;
}

/**
 * API-token sign-in: redeem the one-time ticket from a `/token-login` link,
 * store the session it returns, and go where `useDevLogin` would — a validated
 * `return_to`, else a same-origin `next`, else wherever the user's orgs say.
 *
 * Two things differ from dev-login, both because this can *replace* a session
 * rather than only open one:
 *
 * - The previous user's state is torn down first, with the same teardown
 *   `logout` runs (`clearLocalSession`) minus the server round-trip: the
 *   redeem has already replaced the `oxy_session` cookie, and `/logout` would
 *   clear the new one. It runs only once the ticket has redeemed, so a refused
 *   link leaves whoever was signed in untouched, and before the destination is
 *   resolved, so an invite the previous user left pending in `sessionStorage`
 *   is not followed as the token's user.
 * - It leaves by a full page load, never a soft navigation. Clearing storage
 *   does not clear memory: a store persisted per user (the IDE branch
 *   selection, for one) was restored from the previous user's storage when
 *   this page loaded, and would both show that selection to the new session
 *   and write it straight back on its next update. A reload rebuilds the
 *   stores, and the React Query cache, from the clean slate — the same reason
 *   the axios 401 handler hard-navigates to `/login`.
 */
export const useTokenLogin = ({ returnTo, next, onFailure }: TokenLoginOptions = {}) => {
  const { login } = useAuth();

  return useMutation<AuthResponse, Error, string>({
    mutationFn: AuthService.redeemBrowserTicket,
    onError: (error) => onFailure?.(error),
    onSuccess: async (data) => {
      clearLocalSession();
      login(data.token, data.user);

      const destination = await resolvePostLoginDestination(data, { returnTo, next });
      leaveTo(destination.kind === "external" ? destination.url : destination.path);
    }
  });
};
