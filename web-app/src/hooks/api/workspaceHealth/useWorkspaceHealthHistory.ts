import { useQuery } from "@tanstack/react-query";
import { WorkspaceHealthService } from "@/services/api/workspaceHealth";
import queryKeys from "../queryKey";

/** How far back the Health tab looks. The server keeps ninety days. */
export const HEALTH_HISTORY_DAYS = 30;

/**
 * A workspace's status changes, from
 * `GET /admin/workspace-health/{id}/history`.
 *
 * A minute's stale time: a row appears only when an evaluation finds a
 * different status, and evaluations are at least ten minutes apart.
 */
export const useWorkspaceHealthHistory = (workspaceId: string, days = HEALTH_HISTORY_DAYS) =>
  useQuery({
    queryKey: queryKeys.workspaceHealth.history(workspaceId, days),
    queryFn: () => WorkspaceHealthService.history(workspaceId, days),
    staleTime: 60_000
  });
