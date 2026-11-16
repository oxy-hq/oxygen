import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import type { ExtendApiKeyRequest } from "@/types/apiKey";
import type { TokenSummary } from "@/types/apiToken";
import {
  extendErrorMessage,
  extendSuccessMessage,
  isStaleKeyError,
  tokenNoun
} from "./extendMessages";
import type { ExtendedToken, TokenEndpoints } from "./tokenEndpoints";

export interface ExtendApiKeyVariables {
  /** The row as it was before extending: names it, and tells a revival from an extension. */
  token: TokenSummary;
  request: ExtendApiKeyRequest;
}

/**
 * Push out a token's expiry, through whichever surface `endpoints` names: the caller's own
 * personal tokens, or a service account's. It covered the legacy `/api-keys` routes too until
 * legacy keys were removed.
 */
export const useExtendApiKey = (endpoints: TokenEndpoints) => {
  const queryClient = useQueryClient();

  const invalidate = (id: string) => {
    for (const queryKey of endpoints.keys.lists) queryClient.invalidateQueries({ queryKey });
    queryClient.invalidateQueries({ queryKey: endpoints.keys.activity(id) });
  };

  return useMutation<ExtendedToken, Error, ExtendApiKeyVariables>({
    mutationFn: ({ token, request }) => endpoints.extend(token.id, request),
    onSuccess: (updated, { token }) => {
      invalidate(token.id);
      toast.success(extendSuccessMessage(token, updated));
    },
    onError: (error, { token }) => {
      console.error("Failed to extend:", error);
      // The row on screen is stale (revoked or deleted elsewhere): refetch so it stops offering Extend.
      if (isStaleKeyError(error)) invalidate(token.id);
      toast.error(extendErrorMessage(error, tokenNoun(token)));
    }
  });
};
