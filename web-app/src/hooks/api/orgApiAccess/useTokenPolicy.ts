import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { TokenPolicyService } from "@/services/api/orgApiAccess";
import type { TokenPolicy } from "@/types/orgApiAccess";
import queryKeys from "../queryKey";
import { invalidateApiAccess, retryUnlessClientError } from "./shared";

export const useTokenPolicy = (orgId: string, enabled = true) =>
  useQuery<TokenPolicy>({
    queryKey: queryKeys.org.tokenPolicy(orgId),
    queryFn: () => TokenPolicyService.get(orgId),
    enabled: enabled && !!orgId,
    retry: retryUnlessClientError
  });

/**
 * A policy change can block or unblock tokens that already exist, so the
 * inventory is refreshed along with the policy itself.
 */
export const useUpdateTokenPolicy = () => {
  const queryClient = useQueryClient();
  return useMutation<TokenPolicy, unknown, { orgId: string; policy: TokenPolicy }>({
    mutationFn: ({ orgId, policy }) => TokenPolicyService.put(orgId, policy),
    onSuccess: (saved, { orgId }) => {
      queryClient.setQueryData(queryKeys.org.tokenPolicy(orgId), saved);
      invalidateApiAccess(queryClient, orgId);
    }
  });
};
