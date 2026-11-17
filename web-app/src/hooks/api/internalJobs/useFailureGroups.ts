import { useQuery } from "@tanstack/react-query";
import { InternalJobsService } from "@/services/api/internalJobs";
import queryKeys from "../queryKey";

/** Failed and dead jobs grouped by cause, over the last `days`. Polls with the page. */
export const useFailureGroups = (days: number, options: { paused?: boolean } = {}) =>
  useQuery({
    queryKey: queryKeys.internalJobs.failureGroups(days),
    queryFn: () => InternalJobsService.failureGroups(days),
    refetchInterval: options.paused ? false : 15_000
  });
