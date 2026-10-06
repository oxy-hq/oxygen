import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { OrgTokenService } from "@/services/api/orgApiAccess";
import type { InventoryFilters, InventoryToken } from "@/types/orgApiAccess";
import queryKeys from "../queryKey";
import { retryUnlessClientError, useApiAccessMutation } from "./shared";

/**
 * Every token reaching the org. The filters are applied by the server, and an
 * unfiltered read shares its cache entry with the one the filter menus use for
 * their options — so opening the tab costs one request, not two.
 */
export const useOrgTokens = (orgId: string, filters: InventoryFilters = {}, enabled = true) =>
  useQuery<InventoryToken[]>({
    queryKey: queryKeys.org.tokenInventory(orgId, filters),
    queryFn: () => OrgTokenService.list(orgId, filters),
    enabled: enabled && !!orgId,
    // Changing a filter keeps the current rows on screen until the new ones arrive.
    placeholderData: keepPreviousData,
    retry: retryUnlessClientError
  });

/** Ends a personal token's reach into this org. The token itself is not revoked. */
export const useRevokeOrgTokenGrant = () =>
  useApiAccessMutation((vars: { orgId: string; tokenId: string }) =>
    OrgTokenService.revokeGrant(vars.orgId, vars.tokenId)
  );
