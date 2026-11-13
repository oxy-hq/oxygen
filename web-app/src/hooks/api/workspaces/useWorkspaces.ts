import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { WorkspaceService, type WorkspaceSummary } from "@/services/api/workspaces";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { Workspace } from "@/types/workspace";
import queryKeys from "../queryKey";
import { deriveMaterializingState, shouldRetryWorkspaceQuery } from "./materializing";

export const useWorkspace = (workspaceId: string) => {
  const query = useQuery<Workspace>({
    queryKey: queryKeys.workspaces.item(workspaceId),
    queryFn: () => WorkspaceService.getWorkspace(workspaceId),
    // The working copy isn't on disk YET (pod restart / rolling update). That's
    // a readiness state, not a failure — keep retrying instead of surfacing it,
    // or the shell toasts and redirects on a condition that fixes itself.
    retry: shouldRetryWorkspaceQuery,
    retryDelay: 5_000
  });

  return { ...query, ...deriveMaterializingState(query) };
};

export const useAllWorkspaces = (orgIdOverride?: string) => {
  const storeOrgId = useCurrentOrg((s) => s.org?.id);
  const orgId = orgIdOverride ?? storeOrgId;

  return useQuery<WorkspaceSummary[]>({
    queryKey: queryKeys.workspaces.listByOrg(orgId),
    queryFn: () => {
      if (!orgId) return Promise.resolve([]);
      return WorkspaceService.listAllWorkspaces(orgId);
    },
    enabled: !!orgId,
    // Poll every 3 s while any workspace is still cloning so the UI updates
    // automatically once the background git clone finishes.
    refetchInterval: (query) => {
      const data = query.state.data;
      return data?.some((p) => p.status === "cloning") ? 3000 : false;
    }
  });
};

type DeleteWorkspaceVars = { orgId: string; id: string; deleteFiles?: boolean };
type DeleteWorkspaceContext = {
  previous: WorkspaceSummary[] | undefined;
  listKey: ReturnType<typeof queryKeys.workspaces.listByOrg>;
};

export const useDeleteWorkspace = () => {
  const queryClient = useQueryClient();
  return useMutation<void, Error, DeleteWorkspaceVars, DeleteWorkspaceContext>({
    mutationFn: ({ orgId, id, deleteFiles }) =>
      WorkspaceService.deleteWorkspace(orgId, id, deleteFiles),
    // Synchronous onMutate: setQueryData runs before `mutate()` returns
    // control to the caller, so a `navigate()` immediately after `mutate(...)`
    // mounts OrgDispatcher with the workspace already removed from cache.
    onMutate: ({ orgId, id }) => {
      const listKey = queryKeys.workspaces.listByOrg(orgId);
      const previous = queryClient.getQueryData<WorkspaceSummary[]>(listKey);
      if (previous) {
        queryClient.setQueryData<WorkspaceSummary[]>(
          listKey,
          previous.filter((w) => w.id !== id)
        );
      }
      // Cancel any in-flight list refetch so it can't overwrite the
      // optimistic update before the server confirms. Fire-and-forget:
      // awaiting would defeat onMutate's synchronous timing.
      void queryClient.cancelQueries({ queryKey: listKey });
      return { previous, listKey };
    },
    onError: (_err, _vars, context) => {
      if (context?.previous) {
        queryClient.setQueryData(context.listKey, context.previous);
      }
    },
    onSettled: (_data, _err, { orgId }) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.workspaces.listByOrg(orgId) });
    }
  });
};

export const useRenameWorkspace = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ orgId, id, name }: { orgId: string; id: string; name: string }) =>
      WorkspaceService.renameWorkspace(orgId, id, name),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.workspaces.list() });
    }
  });
};
