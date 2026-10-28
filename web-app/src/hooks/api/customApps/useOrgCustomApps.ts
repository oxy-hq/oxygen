import { useAllWorkspaces } from "@/hooks/api/workspaces/useWorkspaces";
import { navigableWorkspaces } from "@/libs/utils/lastWorkspace";
import type { CustomAppSummary } from "@/types/apps";
import { useWorkspacesCustomApps } from "./useWorkspacesCustomApps";

/**
 * Every published custom app the person can open across the org's workspaces,
 * for the org home's grid and the rail's app marks.
 *
 * Fans out over `useWorkspacesCustomApps` (the same per-workspace list and cache
 * key `useCustomApps` reads), so the server's per-app access filter stays the
 * server's. Only navigable workspaces are asked — one still cloning has no list
 * to give. `currentWorkspaceId` is always asked, so a local-mode box (no org, no
 * workspace list) and a workspace the list has not caught up with still show
 * their own apps.
 *
 * The current workspace's apps come first, then the others in the org's
 * workspace order. An app lives in one workspace, but the list is de-duplicated
 * by id in case a workspace is listed twice.
 *
 * `isPending` is true until there is something to show: the current workspace's
 * own list has settled, and — only while that list is empty — the others have too,
 * so an org whose apps live elsewhere does not flash its empty state. One slow or
 * failing workspace never holds the page up once the current one has answered;
 * `apps` fills in as the rest settle.
 */
export default function useOrgCustomApps(
  currentWorkspaceId: string,
  {
    acrossOrg = true
  }: {
    /** `false`: only the current workspace's apps, the very query (and cache entry)
     *  `useCustomApps` reads, as the rail showed before the org home. */
    acrossOrg?: boolean;
  } = {}
): {
  apps: CustomAppSummary[];
  isPending: boolean;
} {
  const { data: workspaces, isLoading } = useAllWorkspaces();
  const others = acrossOrg
    ? navigableWorkspaces(workspaces ?? [])
        .map((w) => w.id)
        .filter((id) => id !== currentWorkspaceId)
    : [];
  const ids = currentWorkspaceId ? [currentWorkspaceId, ...others] : others;
  // Two calls, so "the current workspace has answered" is its own answer. The
  // lists are the same queries either way, under the same cache keys.
  const current = useWorkspacesCustomApps(currentWorkspaceId ? [currentWorkspaceId] : []);
  const rest = useWorkspacesCustomApps(others);
  const appsByWorkspace = new Map([...current.appsByWorkspace, ...rest.appsByWorkspace]);

  const seen = new Set<string>();
  const apps: CustomAppSummary[] = [];
  for (const id of ids) {
    for (const app of appsByWorkspace.get(id) ?? []) {
      if (seen.has(app.id)) continue;
      seen.add(app.id);
      apps.push(app);
    }
  }
  // `isLoading`, not `isPending`: with no org (local mode) the workspace list is
  // disabled, and a disabled query is pending forever.
  return {
    apps,
    isPending: isLoading || current.isPending || (apps.length === 0 && rest.isPending)
  };
}
