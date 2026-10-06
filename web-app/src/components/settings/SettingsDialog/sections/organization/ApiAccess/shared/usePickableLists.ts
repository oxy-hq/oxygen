import { useOrgAppAccessList } from "@/hooks/api/appAccess";
import { useAllWorkspaces } from "@/hooks/api/workspaces/useWorkspaces";
import type { PickableList } from "./AccessPicker";

/** The org's workspaces, shaped for the access picker and the inventory's filter. */
export function usePickableWorkspaces(orgId: string): PickableList {
  const { data, isPending, isError } = useAllWorkspaces(orgId);
  return {
    items: (data ?? []).map((workspace) => ({ id: workspace.id, name: workspace.name })),
    isPending,
    isError
  };
}

/**
 * The org's custom apps, for "may publish this app". The same list App access
 * edits: every app, including ones the viewer can't personally open — an
 * admin grants publish rights on apps they don't use.
 */
export function usePickableApps(orgId: string, enabled = true): PickableList {
  const { data, isPending, isError } = useOrgAppAccessList(orgId, enabled);
  return {
    items: (data ?? []).map((app) => ({ id: app.id, name: app.name })),
    isPending: enabled && isPending,
    isError
  };
}
