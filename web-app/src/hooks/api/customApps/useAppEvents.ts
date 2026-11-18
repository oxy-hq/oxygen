import { useQuery } from "@tanstack/react-query";
import { AppEventsService } from "@/services/api/appEvents";
import queryKeys from "../queryKey";

/** How often the Events page re-reads: the queue moves, so the numbers should too. */
export const EVENTS_REFRESH_MS = 30_000;

/**
 * Every event type of the app's org, each listener's queue health read live from its
 * deliveries. One read per org: every app of it filters the same reply.
 *
 * The read aggregates every delivery the org has, so only the Events page polls it
 * (`live`). The console's sidebar badge and the Overview's count read it once and keep
 * it for a minute: a badge on the Secrets page is not worth an org-wide aggregation
 * twice a minute.
 */
export function useAppEventTypes(orgSlug: string | undefined, { live = false } = {}) {
  return useQuery({
    queryKey: queryKeys.customApps.eventTypes(orgSlug ?? ""),
    queryFn: () => AppEventsService.types(orgSlug as string),
    enabled: !!orgSlug,
    staleTime: live ? 10_000 : 60_000,
    refetchInterval: live ? EVENTS_REFRESH_MS : false
  });
}

/** One app's dead deliveries in `environment`, grouped by the error they died of. */
export function useAppEventsDead(
  orgSlug: string | undefined,
  appSlug: string | undefined,
  environment: string
) {
  return useQuery({
    queryKey: queryKeys.customApps.eventsDead(orgSlug ?? "", appSlug ?? "", environment),
    queryFn: () => AppEventsService.dead(orgSlug as string, appSlug as string, environment),
    enabled: !!orgSlug && !!appSlug,
    staleTime: 10_000,
    refetchInterval: EVENTS_REFRESH_MS
  });
}
