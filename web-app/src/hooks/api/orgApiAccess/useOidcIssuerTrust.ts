import { useQuery } from "@tanstack/react-query";
import { IssuerTrustPolicyService, OidcIssuerService } from "@/services/api/orgApiAccess";
import type {
  CreateIssuerTrustPolicyRequest,
  CreateOidcIssuerRequest,
  IssuerTrustPolicy,
  OidcIssuer,
  UpdateIssuerTrustPolicyRequest
} from "@/types/orgApiAccess";
import queryKeys from "../queryKey";
import { retryUnlessClientError, useApiAccessMutation } from "./shared";

export const useOidcIssuers = (orgId: string, enabled = true) =>
  useQuery<OidcIssuer[]>({
    queryKey: queryKeys.org.oidcIssuers(orgId),
    queryFn: () => OidcIssuerService.list(orgId),
    enabled: enabled && !!orgId,
    retry: retryUnlessClientError
  });

export const useCreateOidcIssuer = () =>
  useApiAccessMutation((vars: { orgId: string; request: CreateOidcIssuerRequest }) =>
    OidcIssuerService.create(vars.orgId, vars.request)
  );

export const useDeleteOidcIssuer = () =>
  useApiAccessMutation((vars: { orgId: string; issuerId: string }) =>
    OidcIssuerService.remove(vars.orgId, vars.issuerId)
  );

/** Every account's issuer trust policies in the org: the settings screen lists them under each issuer. */
export const useIssuerTrustPolicies = (orgId: string, enabled = true) =>
  useQuery<IssuerTrustPolicy[]>({
    queryKey: queryKeys.org.issuerTrustPolicies(orgId),
    queryFn: () => IssuerTrustPolicyService.listOrg(orgId),
    enabled: enabled && !!orgId,
    retry: retryUnlessClientError
  });

export const useCreateIssuerTrustPolicy = () =>
  useApiAccessMutation(
    (vars: { orgId: string; saId: string; request: CreateIssuerTrustPolicyRequest }) =>
      IssuerTrustPolicyService.create(vars.orgId, vars.saId, vars.request)
  );

/** Replaces a policy's grants, or disables or re-enables it. */
export const useUpdateIssuerTrustPolicy = () =>
  useApiAccessMutation(
    (vars: {
      orgId: string;
      saId: string;
      policyId: string;
      request: UpdateIssuerTrustPolicyRequest;
    }) => IssuerTrustPolicyService.update(vars.orgId, vars.saId, vars.policyId, vars.request)
  );

export const useDeleteIssuerTrustPolicy = () =>
  useApiAccessMutation((vars: { orgId: string; saId: string; policyId: string }) =>
    IssuerTrustPolicyService.remove(vars.orgId, vars.saId, vars.policyId)
  );
