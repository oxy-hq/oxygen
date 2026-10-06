import { useQuery } from "@tanstack/react-query";

import { AppIssuesService } from "@/services/api/appIssues";
import queryKeys from "../queryKey";

/** The window the dossier reads: the pager's own lookback, so "new" here and
 *  "new" in a page mean the same week. */
export const ISSUE_WINDOW_DAYS = 7;

/**
 * An app's failures as issues — one per `(function, fingerprint)`.
 *
 * The section and its header badge both call this; react-query dedupes, so the
 * badge costs no second request.
 */
export function useAppIssues(appId: string | undefined, days = ISSUE_WINDOW_DAYS) {
  return useQuery({
    queryKey: queryKeys.customApps.issues(appId ?? "", days),
    queryFn: () => AppIssuesService.list(appId as string, days),
    enabled: !!appId,
    staleTime: 30_000
  });
}
