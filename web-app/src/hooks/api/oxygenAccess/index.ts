import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { OxygenAccessService } from "@/services/api/oxygenAccess";
import type { OxygenAccess } from "@/types/oxygenAccess";
import { invalidateApiAccess, retryUnlessClientError } from "../orgApiAccess/shared";
import queryKeys from "../queryKey";

export const useOxygenAccessList = (orgId: string, enabled = true) =>
  useQuery<OxygenAccess[]>({
    queryKey: queryKeys.org.oxygenAccess(orgId),
    queryFn: () => OxygenAccessService.list(orgId),
    enabled: enabled && !!orgId,
    retry: retryUnlessClientError
  });

export const useOxygenAccess = (orgId: string, appId: string, enabled = true) =>
  useQuery<OxygenAccess>({
    queryKey: queryKeys.org.oxygenAccessOne(orgId, appId),
    queryFn: () => OxygenAccessService.get(orgId, appId),
    enabled: enabled && !!orgId && !!appId,
    retry: retryUnlessClientError
  });

type Vars = { orgId: string; appId: string; actions?: string[] };

/**
 * An approval or a revoke changes the app's row in the token inventory too (a
 * token of kind `app`), so the inventory is refreshed with the list.
 */
const useOxygenAccessMutation = (run: (vars: Vars) => Promise<OxygenAccess>) => {
  const queryClient = useQueryClient();
  return useMutation<OxygenAccess, unknown, Vars>({
    mutationFn: run,
    onSuccess: (saved, { orgId, appId }) => {
      queryClient.setQueryData(queryKeys.org.oxygenAccessOne(orgId, appId), saved);
      queryClient.invalidateQueries({ queryKey: queryKeys.org.oxygenAccess(orgId) });
      invalidateApiAccess(queryClient, orgId);
    }
  });
};

export const useApproveOxygenAccess = () =>
  useOxygenAccessMutation(({ orgId, appId, actions }) =>
    OxygenAccessService.approve(orgId, appId, actions)
  );

export const useRevokeOxygenAccess = () =>
  useOxygenAccessMutation(({ orgId, appId }) => OxygenAccessService.revoke(orgId, appId));
