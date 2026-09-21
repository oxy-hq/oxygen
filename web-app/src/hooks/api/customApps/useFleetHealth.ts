import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { CustomAppsService } from "@/services/api/customApps";
import type { AppHealthRow } from "@/types/apps";
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
export const useFleetHealth = (needsAttention = false, options: { enabled?: boolean } = {}) =>
  useQuery({
    queryKey: queryKeys.customApps.fleetHealth(needsAttention),
    queryFn: () => CustomAppsService.fleetHealth({ needsAttention }),
    refetchInterval: REFRESH_MS,
    staleTime: REFRESH_MS - 5_000,
    // Gated for the console home, which renders this section only for a standing that
    // reaches Custom apps. Defaults to on, so every existing call site is unchanged.
    enabled: options.enabled ?? true
  });

/**
 * The fleet's health, indexed by app id, for joining onto the app registry.
 *
 * `rows` is `undefined` until the first answer arrives, and stays `undefined`
 * when the query fails. Both mean "unknown" to the list, which then shows no
 * verdict at all rather than a borrowed one — an app nobody measured must not
 * look like one somebody did.
 */
export const useAppHealthIndex = () => {
  const { data, isError, isLoading } = useFleetHealth();
  const rows = useMemo(
    () => (data ? new Map<string, AppHealthRow>(data.apps.map((r) => [r.app_id, r])) : undefined),
    [data]
  );
  return {
    rows,
    isLoading,
    isError,
    /** Published apps the endpoint knows of — more than `rows.size` when the
     *  page cap cut the answer short. */
    total: data?.total ?? 0,
    hasMore: data?.has_more ?? false,
    captureConfigured: data?.observability_configured ?? true,
    evaluatedAt: data?.evaluated_at
  };
};
