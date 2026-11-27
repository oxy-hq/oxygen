import { useQuery } from "@tanstack/react-query";

import { CustomAppsService } from "@/services/api/customApps";
import queryKeys from "../queryKey";

/** How often an open app console re-reads the app's builds. */
export const CONSOLE_BUILDS_POLL_MS = 15_000;

/**
 * Versioned build history for a custom app (new publish pipeline).
 * Disabled until an id is available; empty for legacy rows.
 *
 * `watch` re-reads it every {@link CONSOLE_BUILDS_POLL_MS} while the tab is shown: the
 * app console asks for it, because a publish from CI or a terminal moves staging with
 * nothing on the page to say so, and the Checks panel follows the build staging serves.
 * Every other reader of the same cache entry then gets the fresh list too.
 */
export function useAppBuilds(id: string | undefined, { watch = false }: { watch?: boolean } = {}) {
  return useQuery({
    queryKey: queryKeys.customApps.builds(id ?? ""),
    queryFn: () => CustomAppsService.listBuilds(id as string),
    enabled: !!id,
    refetchInterval: watch ? CONSOLE_BUILDS_POLL_MS : false
  });
}
