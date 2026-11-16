import { useQuery } from "@tanstack/react-query";
import { AdminObservabilityService } from "@/services/api/adminObservability";
import queryKeys from "../queryKey";

/** One org's LLM spend over the window, with the dimensions that explain it. */
export const useOrgCost = (orgId: string | undefined, days: number) =>
  useQuery({
    queryKey: queryKeys.adminObservability.orgCost(orgId ?? "", days),
    queryFn: () => AdminObservabilityService.orgCost(orgId as string, days),
    enabled: Boolean(orgId),
    staleTime: 60_000
  });
