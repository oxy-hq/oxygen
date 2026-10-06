import { useQuery } from "@tanstack/react-query";
import { ServiceAccountService } from "@/services/api/orgApiAccess";
import type { Token } from "@/types/apiToken";
import type { CreateServiceAccountTokenRequest } from "@/types/orgApiAccess";
import queryKeys from "../queryKey";
import { retryUnlessClientError, useApiAccessMutation } from "./shared";

export const useServiceAccountTokens = (orgId: string, saId: string, enabled = true) =>
  useQuery<Token[]>({
    queryKey: queryKeys.org.serviceAccountTokens(orgId, saId),
    queryFn: () => ServiceAccountService.listTokens(orgId, saId),
    enabled: enabled && !!orgId && !!saId,
    retry: retryUnlessClientError
  });

type TokenRef = { orgId: string; saId: string; tokenId: string };

export const useCreateServiceAccountToken = () =>
  useApiAccessMutation(
    (vars: { orgId: string; saId: string; request: CreateServiceAccountTokenRequest }) =>
      ServiceAccountService.createToken(vars.orgId, vars.saId, vars.request)
  );

export const useRegenerateServiceAccountToken = () =>
  useApiAccessMutation((vars: TokenRef) =>
    ServiceAccountService.regenerateToken(vars.orgId, vars.saId, vars.tokenId)
  );

export const useRevokeServiceAccountToken = () =>
  useApiAccessMutation((vars: TokenRef) =>
    ServiceAccountService.revokeToken(vars.orgId, vars.saId, vars.tokenId)
  );
