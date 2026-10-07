import { useQuery } from "@tanstack/react-query";
import { StandingTokensService } from "@/services/api/standingTokens";
import queryKeys from "../queryKey";
import {
  listedRevokeError,
  retryUnlessRefused,
  useRevokeListedToken
} from "../staffTokenLists/useRevokeListedToken";

/**
 * Every personal token with staff or partner standing. A 403 is one of three refusals: no
 * `manage_platform_grants`, staff access limited to some organizations, or a session that is not
 * a browser sign-in (a token, or the session `oxyc login-link` opens from one). None changes on
 * asking again, so it is not retried.
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
    // One answer for a token that is gone and one that no longer carries a standing.
    "This token is no longer in this list."
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
