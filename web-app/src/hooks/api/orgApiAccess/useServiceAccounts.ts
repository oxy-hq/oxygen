import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ServiceAccountService } from "@/services/api/orgApiAccess";
import type {
  CreateServiceAccountRequest,
  ServiceAccount,
  UpdateServiceAccountRequest
} from "@/types/orgApiAccess";
import queryKeys from "../queryKey";
import { invalidateApiAccess, retryUnlessClientError, useApiAccessMutation } from "./shared";

export const useServiceAccounts = (orgId: string, enabled = true) =>
  useQuery<ServiceAccount[]>({
    queryKey: queryKeys.org.serviceAccounts(orgId),
    queryFn: () => ServiceAccountService.list(orgId),
    enabled: enabled && !!orgId,
    retry: retryUnlessClientError
  });

/**
 * Seeds the list with the new account before the refetch lands, so the caller
 * can open the account's page straight away instead of waiting on the list.
 */
export const useCreateServiceAccount = () => {
  const queryClient = useQueryClient();
  return useMutation<
    ServiceAccount,
    unknown,
    { orgId: string; request: CreateServiceAccountRequest }
  >({
    mutationFn: ({ orgId, request }) => ServiceAccountService.create(orgId, request),
    onSuccess: (created, { orgId }) => {
      queryClient.setQueryData<ServiceAccount[]>(queryKeys.org.serviceAccounts(orgId), (list) =>
        list?.some((a) => a.id === created.id) ? list : [...(list ?? []), created]
      );
      invalidateApiAccess(queryClient, orgId);
    }
  });
};

/** Edits the description or role, and disables or re-enables the account. */
export const useUpdateServiceAccount = () =>
  useApiAccessMutation(
    (vars: { orgId: string; saId: string; request: UpdateServiceAccountRequest }) =>
      ServiceAccountService.update(vars.orgId, vars.saId, vars.request)
  );

export const useDeleteServiceAccount = () =>
  useApiAccessMutation((vars: { orgId: string; saId: string }) =>
    ServiceAccountService.remove(vars.orgId, vars.saId)
  );
