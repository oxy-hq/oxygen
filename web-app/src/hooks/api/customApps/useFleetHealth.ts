import { useQuery } from "@tanstack/react-query";
import { CustomAppsService } from "@/services/api/customApps";
import queryKeys from "../queryKey";

/** How often the fleet table re-asks. */
const REFRESH_MS = 60_000;

/**
 * Fleet health for every published app the caller can see.
 *
 * Polls rather than waiting for a manual refresh: this is the page someone
 * leaves open on a second monitor, and a health table that silently goes stale
 * is the same failure it exists to catch. The response carries `evaluated_at`
 * so the UI can show its own age — without that, "nothing has changed" and
 * "nothing is running" look identical.
 *
 * `staleTime` sits just under the interval so a tab switch repaints from cache
 * instead of flashing a skeleton at a table the operator was already reading.
 */
export const useFleetHealth = (needsAttention = false) =>
  useQuery({
    queryKey: queryKeys.customApps.fleetHealth(needsAttention),
    queryFn: () => CustomAppsService.fleetHealth({ needsAttention }),
    refetchInterval: REFRESH_MS,
    staleTime: REFRESH_MS - 5_000
  });
