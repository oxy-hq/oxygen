import { useMemo } from "react";
import type { TokenActivityEndpoints, TokenEndpoints } from "@/hooks/api/apiKeys/tokenEndpoints";
import { OrgTokenService, ServiceAccountService } from "@/services/api/orgApiAccess";
import queryKeys from "../queryKey";

/**
 * `/orgs/{org_id}/service-accounts/{sa_id}/tokens/{id}/…`: one service account's tokens, for the
 * shared Extend popover and Activity drawer.
 */
export const serviceAccountTokenEndpoints = (orgId: string, saId: string): TokenEndpoints => ({
  extend: (id, request) => ServiceAccountService.extendToken(orgId, saId, id, request),
  activity: (id, limit) => ServiceAccountService.tokenActivity(orgId, saId, id, limit),
  keys: {
    // The account's token list sits under `serviceAccounts(orgId)`; the inventory lists it too.
    lists: [queryKeys.org.serviceAccounts(orgId), queryKeys.org.tokenInventoryAll(orgId)],
    activity: (id) => queryKeys.org.serviceAccountTokenActivity(orgId, saId, id)
  }
});

export const useServiceAccountTokenEndpoints = (orgId: string, saId: string): TokenEndpoints =>
  useMemo(() => serviceAccountTokenEndpoints(orgId, saId), [orgId, saId]);

/**
 * `/orgs/{org_id}/tokens/{id}/activity`: what any token reaching the org did *in this org*. The
 * inventory is read-only, so there is nothing to extend.
 */
export const orgInventoryEndpoints = (orgId: string): TokenActivityEndpoints => ({
  activity: (id, limit) => OrgTokenService.activity(orgId, id, limit),
  keys: {
    activity: (id) => queryKeys.org.tokenInventoryActivity(orgId, id)
  }
});

export const useOrgInventoryEndpoints = (orgId: string): TokenActivityEndpoints =>
  useMemo(() => orgInventoryEndpoints(orgId), [orgId]);
