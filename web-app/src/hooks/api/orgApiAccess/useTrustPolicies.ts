import { useQuery } from "@tanstack/react-query";
import { TrustPolicyService } from "@/services/api/orgApiAccess";
import type {
  CreateTrustPolicyRequest,
  TrustPolicy,
  UpdateTrustPolicyRequest
} from "@/types/orgApiAccess";
import queryKeys from "../queryKey";
import { retryUnlessClientError, useApiAccessMutation } from "./shared";

export const useTrustPolicies = (orgId: string, saId: string, enabled = true) =>
  useQuery<TrustPolicy[]>({
    queryKey: queryKeys.org.trustPolicies(orgId, saId),
    queryFn: () => TrustPolicyService.list(orgId, saId),
    enabled: enabled && !!orgId && !!saId,
    retry: retryUnlessClientError
  });

export const useCreateTrustPolicy = () =>
  useApiAccessMutation((vars: { orgId: string; saId: string; request: CreateTrustPolicyRequest }) =>
    TrustPolicyService.create(vars.orgId, vars.saId, vars.request)
  );

/** Edits a policy's match rules or grants, and disables or re-enables it. */
export const useUpdateTrustPolicy = () =>
  useApiAccessMutation(
    (vars: { orgId: string; saId: string; policyId: string; request: UpdateTrustPolicyRequest }) =>
      TrustPolicyService.update(vars.orgId, vars.saId, vars.policyId, vars.request)
  );

export const useDeleteTrustPolicy = () =>
  useApiAccessMutation((vars: { orgId: string; saId: string; policyId: string }) =>
    TrustPolicyService.remove(vars.orgId, vars.saId, vars.policyId)
  );
