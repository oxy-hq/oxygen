import { useQuery } from "@tanstack/react-query";
import { AdminObservabilityService } from "@/services/api/adminObservability";
import queryKeys from "../queryKey";

/**
 * One row per org: spend, app traffic and the latency tail.
 *
 * `enabled: false` reads the cache only: a cached fleet is still returned,
 * and nothing is fetched. The org console uses it so one org's page does not
 * pay for a fleet-wide read unless the operator asks for its rank.
 */
export const useObservabilityFleet = (
  days: number,
  { enabled = true }: { enabled?: boolean } = {}
) =>
  useQuery({
    queryKey: queryKeys.adminObservability.fleet(days),
    queryFn: () => AdminObservabilityService.fleet(days),
    staleTime: 60_000,
    enabled
  });
