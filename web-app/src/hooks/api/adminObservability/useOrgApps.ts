import { useQuery } from "@tanstack/react-query";
import { AdminObservabilityService } from "@/services/api/adminObservability";
import queryKeys from "../queryKey";

/** One org's custom-app performance over the window. */
export const useOrgApps = (orgId: string | undefined, days: number) =>
  useQuery({
    queryKey: queryKeys.adminObservability.orgApps(orgId ?? "", days),
    queryFn: () => AdminObservabilityService.orgApps(orgId as string, days),
    enabled: Boolean(orgId),
    staleTime: 60_000
  });
