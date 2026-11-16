import { useQuery } from "@tanstack/react-query";
import { AdminObservabilityService } from "@/services/api/adminObservability";
import queryKeys from "../queryKey";

/** What one org consumes, meter by meter, with how far each figure can be trusted. */
export const useOrgResources = (orgId: string | undefined, days: number) =>
  useQuery({
    queryKey: queryKeys.adminObservability.orgResources(orgId ?? "", days),
    queryFn: () => AdminObservabilityService.orgResources(orgId as string, days),
    enabled: Boolean(orgId),
    staleTime: 60_000
  });
