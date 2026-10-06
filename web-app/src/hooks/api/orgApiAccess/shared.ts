import { type QueryClient, useMutation, useQueryClient } from "@tanstack/react-query";
import { apiStatus } from "@/libs/apiError";
import queryKeys from "../queryKey";

/**
 * A 4xx is an answer, not a blip: 404 means the server predates this feature
 * (or the row is gone) and 403 means the caller isn't an org admin. Retrying
 * either only delays the calm state the section renders for it.
 */
export const retryUnlessClientError = (failureCount: number, error: unknown): boolean => {
  const status = apiStatus(error);
  if (status !== undefined && status >= 400 && status < 500) return false;
  return failureCount < 2;
};

/**
 * Service accounts, their tokens and their policies all feed the inventory
 * and each other's counts, so every write refreshes both lineages.
 */
export const invalidateApiAccess = (queryClient: QueryClient, orgId: string) => {
  queryClient.invalidateQueries({ queryKey: queryKeys.org.serviceAccounts(orgId) });
  queryClient.invalidateQueries({ queryKey: queryKeys.org.tokenInventoryAll(orgId) });
};

/** A mutation on anything under Organization → API access. Callers own the toast. */
export const useApiAccessMutation = <TVars extends { orgId: string }, TData>(
  mutationFn: (vars: TVars) => Promise<TData>
) => {
  const queryClient = useQueryClient();
  return useMutation<TData, unknown, TVars>({
    mutationFn,
    onSuccess: (_data, vars) => invalidateApiAccess(queryClient, vars.orgId)
  });
};
