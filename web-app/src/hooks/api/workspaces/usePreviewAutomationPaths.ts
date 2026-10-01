import { useQuery } from "@tanstack/react-query";
import { AutomationService } from "@/services/api/automations";
import queryKeys from "../queryKey";

/**
 * `.procedure.yml` / `.automation.yml` paths on a preview's branch, offered
 * as suggestions in the "Dry-run a procedure" field — a free-text path is
 * always accepted, this only saves typing it out.
 *
 * Deliberately NOT `useAutomations` (`src/hooks/api/automations/useAutomations.ts`):
 * that hook calls `useCurrentProjectBranch()`, which throws outside the IDE
 * route, and the Previews tab lives in Settings. This calls the same
 * `AutomationService.listAutomations(id, branch)` it wraps directly, against
 * the workspace id (a preview compiles a workspace's one project) and the
 * previewed branch rather than whatever the IDE has open.
 *
 * Best-effort: a failure here just means no suggestions, so the field falls
 * back to free text — errors are swallowed rather than surfaced.
 */
export default function usePreviewAutomationPaths(
  workspaceId: string | undefined,
  branch: string | undefined
) {
  const query = useQuery({
    queryKey: queryKeys.workspaces.previewAutomationPaths(workspaceId ?? "", branch ?? ""),
    queryFn: () => AutomationService.listAutomations(workspaceId ?? "", branch ?? ""),
    enabled: !!workspaceId && !!branch,
    retry: false
  });
  return query.data?.map((file) => file.path) ?? [];
}
