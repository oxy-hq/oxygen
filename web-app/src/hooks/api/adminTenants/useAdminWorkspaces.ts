import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { isAxiosError } from "axios";
import { toast } from "sonner";
import { AdminWorkspacesService, type ListWorkspacesQuery } from "@/services/api/adminTenants";
import queryKeys from "../queryKey";

function errMessage(err: unknown, fallback: string): string {
  if (isAxiosError(err)) return err.response?.data?.message ?? err.message;
  if (err instanceof Error) return err.message;
  return fallback;
}

/**
 * `keepPreviousData` is opt-in for search-as-you-type callers: without it a
 * changed `search` term is a new query key, so `data` drops to `undefined` and
 * the list a picker is rendering blinks out and back on every keystroke. Off by
 * default — a list page that changes filters should show the new filter's
 * loading state, not the old filter's rows.
 */
export const useAdminWorkspacesList = (
  query: ListWorkspacesQuery = {},
  options: { enabled?: boolean; keepPreviousData?: boolean } = {}
) =>
  useQuery({
    queryKey: queryKeys.adminWorkspaces.list(query.search, query.status, query.org_id),
    queryFn: () => AdminWorkspacesService.list(query),
    enabled: options.enabled ?? true,
    placeholderData: options.keepPreviousData ? (prev) => prev : undefined
  });

/** The server's largest page (`page_size` is clamped to 200). */
export const PICKER_PAGE_SIZE = 200;

/**
 * Workspaces for the "Open Factory…" picker: with no `search`, the newest page,
 * which the picker filters by org and workspace name in the browser; with one, the
 * server's search by workspace name across every workspace, so one older than the
 * newest page is still found. Kept on its own key so it never shares a cache entry
 * with the staff console's paged list.
 */
export const useAdminWorkspacesForPicker = (enabled: boolean, search = "") =>
  useQuery({
    queryKey: queryKeys.adminWorkspaces.picker(search),
    queryFn: () =>
      AdminWorkspacesService.list({
        page_size: PICKER_PAGE_SIZE,
        ...(search ? { search } : {})
      }),
    enabled,
    staleTime: 60_000,
    placeholderData: (prev) => prev
  });

export const useAdminWorkspaceDetail = (workspaceId: string | undefined) =>
  useQuery({
    queryKey: queryKeys.adminWorkspaces.detail(workspaceId ?? ""),
    queryFn: () => AdminWorkspacesService.detail(workspaceId as string),
    enabled: !!workspaceId
  });

export const useRenameAdminWorkspace = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ workspaceId, name }: { workspaceId: string; name: string }) =>
      AdminWorkspacesService.rename(workspaceId, name),
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: queryKeys.adminWorkspaces.all });
      qc.invalidateQueries({
        queryKey: queryKeys.adminWorkspaces.detail(vars.workspaceId)
      });
      toast.success("Workspace renamed");
    },
    onError: (err) => toast.error(errMessage(err, "Failed to rename workspace"))
  });
};

export const useDeleteAdminWorkspace = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (workspaceId: string) => AdminWorkspacesService.remove(workspaceId),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.adminWorkspaces.all });
      toast.success("Workspace deleted");
    },
    onError: (err) => toast.error(errMessage(err, "Failed to delete workspace"))
  });
};

export const useAdminFactoryAccess = (workspaceId: string | undefined) =>
  useQuery({
    queryKey: queryKeys.adminWorkspaces.factoryAccess(workspaceId ?? ""),
    queryFn: () => AdminWorkspacesService.factoryAccess(workspaceId as string),
    enabled: !!workspaceId
  });

export const useSetAdminFactoryAccess = (workspaceId: string) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ userId, granted }: { userId: string; granted: boolean }) =>
      AdminWorkspacesService.setFactoryAccess(workspaceId, userId, granted),
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: queryKeys.adminWorkspaces.factoryAccess(workspaceId) });
      toast.success(vars.granted ? "Factory access granted" : "Factory access revoked");
    },
    onError: (err) => toast.error(errMessage(err, "Failed to change Factory access"))
  });
};
