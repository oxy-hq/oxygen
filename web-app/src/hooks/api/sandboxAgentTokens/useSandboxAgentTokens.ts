import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { apiStatus } from "@/libs/apiError";
import { SandboxAgentTokensService } from "@/services/api/sandboxAgentTokens";
import type { Token } from "@/types/apiToken";
import queryKeys from "../queryKey";

/**
 * Every sandbox agent token the caller's staff access reaches. A 403 is the capability gate
 * (`operate_platform`): asking again cannot change it, so it is not retried.
 */
export const useSandboxAgentTokens = () =>
  useQuery({
    queryKey: queryKeys.sandboxAgentTokens.list(),
    queryFn: SandboxAgentTokensService.list,
    retry: (failureCount, error) => apiStatus(error) !== 403 && failureCount < 3
  });

/**
 * What a failed revoke says. `null` for a 403: the API client already said "You don't have
 * permission to do this." for it.
 */
export const revokeErrorMessage = (error: unknown): string | null => {
  switch (apiStatus(error)) {
    case 403:
      return null;
    case 404:
      // One answer for a token that is gone and one outside the caller's orgs.
      return "This token no longer exists, or it is outside the organizations your access covers.";
    default:
      return "Couldn't revoke the token. Try again.";
  }
};

/**
 * Revoke one, whoever minted it. The list is read again whether it worked or not: a refusal means
 * the row on screen was stale. The minter's own list in Settings is read again too.
 */
export const useRevokeSandboxAgentToken = () => {
  const queryClient = useQueryClient();
  return useMutation<Token, unknown, Pick<Token, "id" | "name">>({
    mutationFn: ({ id }) => SandboxAgentTokensService.revoke(id),
    onSuccess: (_token, { name }) => {
      toast.success(`Revoked "${name}"`);
    },
    onError: (error) => {
      const message = revokeErrorMessage(error);
      if (message) toast.error(message);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.sandboxAgentTokens.all });
      queryClient.invalidateQueries({ queryKey: queryKeys.userToken.all });
    }
  });
};
