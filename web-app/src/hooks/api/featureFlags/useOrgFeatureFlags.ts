import { useQuery } from "@tanstack/react-query";
import { FeatureFlagsService } from "@/services/api/featureFlags";
import queryKeys from "../queryKey";

/** The `client` flags the server evaluated for the caller in this org. */
export const useOrgFeatureFlags = (orgId: string | undefined) =>
  useQuery({
    queryKey: queryKeys.featureFlags.forOrg(orgId ?? ""),
    queryFn: () => FeatureFlagsService.forOrg(orgId as string),
    enabled: !!orgId,
    staleTime: 60_000
  });
