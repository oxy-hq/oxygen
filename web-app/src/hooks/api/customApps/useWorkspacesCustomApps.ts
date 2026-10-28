import { useQueries } from "@tanstack/react-query";
import { CustomAppsService } from "@/services/api/customApps";
import type { CustomAppSummary } from "@/types/apps";
import queryKeys from "../queryKey";

export interface WorkspacesCustomApps {
  /** Every requested id is present; a list that failed reads as `[]`. */
  appsByWorkspace: ReadonlyMap<string, CustomAppSummary[]>;
  /** True until every requested list has settled. `false` for no ids. */
  isPending: boolean;
}

/**
 * Published custom apps for several workspaces at once — the org-wide view a
 * member can reach. There is no member-reachable org-level list (the
 * `/orgs/{id}/apps` one is OrgAdmin), so this fans out over the same
 * `/{workspaceId}/custom-apps` list `useCustomApps` reads, under the same cache
 * key: the per-app scope filter stays the server's, and a landing that counted
 * a workspace's apps hands HQ a warm cache.
 *
 * A list that fails reads as no apps. Every caller is discovery — which
 * workspace to land in, which apps to point at — where a missing card is a
 * recoverable annoyance and a stuck spinner is not. One retry, not the default
 * three: the landing waits on these, and the default backoff is seconds.
 */
export const useWorkspacesCustomApps = (workspaceIds: readonly string[]): WorkspacesCustomApps => {
  const results = useQueries({
    queries: workspaceIds.map((id) => ({
      queryKey: queryKeys.customApps.list(id),
      queryFn: () => CustomAppsService.listForWorkspace(id),
      retry: 1,
      // The org home and the rail read N lists on every page; without this each
      // remount or window focus would refetch all of them.
      staleTime: 60_000
    }))
  });
  return {
    appsByWorkspace: new Map(workspaceIds.map((id, i) => [id, results[i]?.data ?? []])),
    isPending: results.some((r) => r.isPending)
  };
};
