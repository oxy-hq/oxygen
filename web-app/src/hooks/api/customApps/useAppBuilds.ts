import { useQuery } from "@tanstack/react-query";

import { CustomAppsService } from "@/services/api/customApps";
import queryKeys from "../queryKey";

/**
 * Versioned build history for a custom app (new publish pipeline).
 * Disabled until an id is available; empty for legacy rows.
 */
export function useAppBuilds(id: string | undefined) {
  return useQuery({
    queryKey: queryKeys.customApps.builds(id ?? ""),
    queryFn: () => CustomAppsService.listBuilds(id as string),
    enabled: !!id
  });
}
