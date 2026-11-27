import { useQuery } from "@tanstack/react-query";
import { AppReleasesService } from "@/services/api/appReleases";
import queryKeys from "../queryKey";

/** The windows the timeline offers. */
export const RELEASE_WINDOWS = [1, 7, 30] as const;
export type ReleaseWindow = (typeof RELEASE_WINDOWS)[number];
export const DEFAULT_RELEASE_WINDOW: ReleaseWindow = 7;

/**
 * The release timeline of one app. Under the `customApps` key prefix, so a
 * production move (`useMoveProduction` invalidates `customApps.all()`) refetches
 * it. A minute of stale time: each read runs percentiles over the window, and a
 * window focus should not re-run them.
 */
export function useAppReleases(appId: string, days: ReleaseWindow, enabled = true) {
  return useQuery({
    queryKey: queryKeys.customApps.releases(appId, days),
    queryFn: () => AppReleasesService.list(appId, days),
    enabled: enabled && !!appId,
    staleTime: 60_000
  });
}
