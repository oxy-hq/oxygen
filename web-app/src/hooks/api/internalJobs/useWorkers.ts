import { useQuery } from "@tanstack/react-query";
import { InternalJobsService } from "@/services/api/internalJobs";
import queryKeys from "../queryKey";

const DEFAULT_INTERVAL_MS = 5_000;

export const useWorkers = (
  options: { paused?: boolean; intervalMs?: number; enabled?: boolean } = {}
) => {
  const { paused = false, intervalMs = DEFAULT_INTERVAL_MS, enabled = true } = options;
  return useQuery({
    queryKey: queryKeys.internalJobs.workers(),
    queryFn: () => InternalJobsService.workers(),
    refetchInterval: paused ? false : intervalMs,
    enabled
  });
};
