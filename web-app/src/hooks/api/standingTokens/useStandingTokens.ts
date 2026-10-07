import { useQuery } from "@tanstack/react-query";
import { StandingTokensService } from "@/services/api/standingTokens";
import queryKeys from "../queryKey";
import {
  listedRevokeError,
  retryUnlessRefused,
  useRevokeListedToken
} from "../staffTokenLists/useRevokeListedToken";

/**
 * Every personal token with staff or partner standing that the caller's staff access covers. A 403
 * is the capability gate (`manage_platform_grants`), or a caller signed in with an API token where
 * the route wants a browser session: neither changes on asking again, so it is not retried.
 */
export const useStandingTokens = () =>
  useQuery({
    queryKey: queryKeys.standingTokens.list(),
    queryFn: StandingTokensService.list,
    retry: retryUnlessRefused
  });

/**
 * What a failed revoke says. `null` for a 403: the API client already said "You don't have
 * permission to do this." for it.
 */
export const revokeErrorMessage = (error: unknown): string | null =>
  listedRevokeError(
    error,
    // One answer for a token that is gone, one that no longer carries standing, and one outside
    // what the caller's access covers.
    "This token is no longer in this list, or it is outside what your staff access covers."
  );

/**
 * Revoke one, whoever owns it. The list is read again whether it worked or not: a refusal means
 * the row on screen was stale. The owner's own list in Settings is read again too.
 */
export const useRevokeStandingToken = () =>
  useRevokeListedToken({
    revoke: StandingTokensService.revoke,
    listKey: queryKeys.standingTokens.all,
    errorMessage: revokeErrorMessage
  });
