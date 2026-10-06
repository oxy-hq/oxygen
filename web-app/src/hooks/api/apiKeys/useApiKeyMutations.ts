import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { ApiKeyService } from "@/services/api/apiKey";
import type { ExtendApiKeyRequest } from "@/types/apiKey";
import type { TokenSummary } from "@/types/apiToken";
import queryKeys from "../queryKey";
import {
  extendErrorMessage,
  extendSuccessMessage,
  isStaleKeyError,
  tokenNoun
} from "./extendMessages";
import type { ExtendedToken, TokenEndpoints } from "./tokenEndpoints";

/** Revoke a legacy API key, on the legacy `/{workspaceId}/api-keys` routes. */
export const useRevokeApiKey = () => {
  const queryClient = useQueryClient();
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;

  return useMutation<void, Error, string>({
    mutationFn: (id) => ApiKeyService.revokeApiKey(projectId, id),
    onSuccess: () => {
      // The legacy list is the only one a legacy API key is in: no token list returns it.
      queryClient.invalidateQueries({ queryKey: queryKeys.apiKey.list(projectId) });
      toast.success("Legacy API key revoked");
    },
    onError: (error) => {
      console.error("Failed to revoke legacy API key:", error);
      toast.error("Couldn't revoke the legacy API key");
    }
  });
};

export interface ExtendApiKeyVariables {
  /** The row as it was before extending: names it, and tells a revival from an extension. */
  token: TokenSummary;
  request: ExtendApiKeyRequest;
}

/**
 * Extend a legacy API key or a token, through whichever surface `endpoints` names (workspace
 * legacy keys, personal tokens, a service account's tokens).
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
