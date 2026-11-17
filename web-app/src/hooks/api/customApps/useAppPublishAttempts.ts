import { useQuery } from "@tanstack/react-query";

import { CustomAppsService } from "@/services/api/customApps";
import queryKeys from "../queryKey";

/**
 * An app's publish attempts, newest first: the failed ones beside the ones that
 * landed a build, each with its stage timings. Under the `customApps` prefix, so a
 * publish or rollback elsewhere in the console refetches it.
 */
export function useAppPublishAttempts(id: string | undefined) {
  return useQuery({
    queryKey: queryKeys.customApps.publishAttempts(id ?? ""),
    queryFn: () => CustomAppsService.listPublishAttempts(id as string),
    enabled: !!id
  });
}
