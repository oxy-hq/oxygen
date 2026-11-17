import { useQuery } from "@tanstack/react-query";

import { CustomAppsService } from "@/services/api/customApps";
import { PRODUCTION } from "@/types/appEnvironments";
import { isForbidden } from "../httpStatus";
import queryKeys from "../queryKey";

/** Recorded traces settle once and are pruned after days, so a minute-old read
 *  is as good as a fresh one for a map. */
const RECENT_TRACES_STALE_MS = 60_000;

/**
 * Each function's latest recorded runs in `environment` (production when absent),
 * folded to what each touched: the app map's right-hand side. One request for the
 * whole app. `enabled` lets the panel hold it until it is on screen.
 *
 * A 403 is an answer (no reach to that environment), so it is not retried.
 */
export function useRecentTraces(id: string | undefined, environment?: string, enabled = true) {
  const env = environment && environment !== PRODUCTION ? environment : undefined;
  return useQuery({
    queryKey: queryKeys.customApps.recentTraces(id ?? "", env),
    queryFn: () => CustomAppsService.listRecentTraces(id as string, env),
    enabled: !!id && enabled,
    staleTime: RECENT_TRACES_STALE_MS,
    retry: (failures, error) => !isForbidden(error) && failures < 3
  });
}
