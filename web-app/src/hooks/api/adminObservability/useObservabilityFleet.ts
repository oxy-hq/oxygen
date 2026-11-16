import { useQuery } from "@tanstack/react-query";
import { AdminObservabilityService } from "@/services/api/adminObservability";
import queryKeys from "../queryKey";

/** One row per org: spend, app traffic and the latency tail. */
export const useObservabilityFleet = (days: number) =>
  useQuery({
    queryKey: queryKeys.adminObservability.fleet(days),
    queryFn: () => AdminObservabilityService.fleet(days),
    staleTime: 60_000
  });
