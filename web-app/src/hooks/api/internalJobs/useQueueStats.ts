import { useQuery } from "@tanstack/react-query";
import { InternalJobsService } from "@/services/api/internalJobs";
import queryKeys from "../queryKey";

const DEFAULT_INTERVAL_MS = 5_000;

/**
 * Polls `/admin/internal-jobs/queue-stats`. The operator console
 * drives the cadence — keep it at 5s by default for the "alive" feel,
 * accept a paused flag from the LiveIndicator so the user can stop
 * background fetches without unmounting the page.
 */
export const useQueueStats = (
  options: { paused?: boolean; intervalMs?: number; enabled?: boolean } = {}
) => {
  const { paused = false, intervalMs = DEFAULT_INTERVAL_MS, enabled = true } = options;
  return useQuery({
    queryKey: queryKeys.internalJobs.queueStats(),
    queryFn: () => InternalJobsService.queueStats(),
    refetchInterval: paused ? false : intervalMs,
    enabled
  });
};
