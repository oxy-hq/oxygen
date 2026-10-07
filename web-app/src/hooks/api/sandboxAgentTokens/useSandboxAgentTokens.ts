import { useQuery } from "@tanstack/react-query";
import { SandboxAgentTokensService } from "@/services/api/sandboxAgentTokens";
import queryKeys from "../queryKey";
import {
  listedRevokeError,
  retryUnlessRefused,
  useRevokeListedToken
} from "../staffTokenLists/useRevokeListedToken";

/**
 * Every sandbox agent token the caller's staff access reaches. A 403 is the capability gate
 * (`operate_platform`): asking again cannot change it, so it is not retried.
 */
export const useSandboxAgentTokens = () =>
  useQuery({
    queryKey: queryKeys.sandboxAgentTokens.list(),
    queryFn: SandboxAgentTokensService.list,
    retry: retryUnlessRefused
  });

/**
 * What a failed revoke says. `null` for a 403: the API client already said "You don't have
 * permission to do this." for it.
 */
export const revokeErrorMessage = (error: unknown): string | null =>
  listedRevokeError(
    error,
    // One answer for a token that is gone and one outside the caller's orgs.
    "This token no longer exists, or it is outside the organizations your access covers."
  );

/**
 * Revoke one, whoever minted it. The list is read again whether it worked or not: a refusal means
 * the row on screen was stale. The minter's own list in Settings is read again too.
 */
export const useRevokeSandboxAgentToken = () =>
  useRevokeListedToken({
    revoke: SandboxAgentTokensService.revoke,
    listKey: queryKeys.sandboxAgentTokens.all,
    errorMessage: revokeErrorMessage
  });
