import { useQuery } from "@tanstack/react-query";
import { WorkspaceHealthService } from "@/services/api/workspaceHealth";
import queryKeys from "../queryKey";

/**
 * Fetches the cross-tenant workspace health rollup from
 * `GET /admin/workspace-health`. Results are sorted worst-first by the
 * backend. Stale time is short (30s) — this is an operator console
 * surface where freshness matters.
 */
export const useWorkspaceHealth = (options: { enabled?: boolean } = {}) =>
  useQuery({
    queryKey: queryKeys.workspaceHealth.list(),
    queryFn: () => WorkspaceHealthService.list(),
    staleTime: 30_000,
    // The console home asks for this only when the operator's standing reaches the
    // health page; without the gate a narrow staff grant lands on `/admin` and collects
    // a 403 for a section it was never shown.
    enabled: options.enabled ?? true
  });
