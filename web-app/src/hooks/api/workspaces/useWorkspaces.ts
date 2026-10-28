import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import { WorkspaceService, type WorkspaceSummary } from "@/services/api/workspaces";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { Workspace, WorkspaceBranchesResponse } from "@/types/workspace";
import queryKeys from "../queryKey";
import { deriveMaterializingState, shouldRetryWorkspaceQuery } from "./materializing";

export const useWorkspace = (workspaceId: string) => {
  // Apps-first: no `/git-state` for someone who may not use the Factory.
  const gitStateOnlyForFactory = useAppsFirstExperience();
  const query = useQuery<Workspace>({
    queryKey: queryKeys.workspaces.item(workspaceId),
    queryFn: () => WorkspaceService.getWorkspace(workspaceId, { gitStateOnlyForFactory }),
    // The working copy isn't on disk YET (pod restart / rolling update). That's
    // a readiness state, not a failure — keep retrying instead of surfacing it,
    // or the shell toasts and redirects on a condition that fixes itself.
    retry: shouldRetryWorkspaceQuery,
    retryDelay: 5_000
  });

  return { ...query, ...deriveMaterializingState(query) };
};

export const useWorkspaceBranches = (workspaceId: string) => {
  return useQuery<WorkspaceBranchesResponse>({
    queryKey: queryKeys.workspaces.branches(workspaceId),
    queryFn: () => WorkspaceService.getWorkspaceBranches(workspaceId),
    enabled: !!workspaceId
  });
};

export const useSwitchWorkspaceBranch = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      workspaceId,
      branchName,
      baseBranch
    }: {
      workspaceId: string;
      branchName: string;
      baseBranch?: string;
    }) => WorkspaceService.switchWorkspaceBranch(workspaceId, branchName, baseBranch),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.item(variables.workspaceId)
      });
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.branches(variables.workspaceId)
      });
    }
  });
};

export const usePullChanges = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ workspaceId, branchName }: { workspaceId: string; branchName: string }) =>
      WorkspaceService.pullChanges(workspaceId, branchName),
    onSuccess: (_, variables) => {
      // `refetchType: "all"` covers inactive observers so the status
      // updates even if BranchInfo unmounts during navigation.
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.revisionInfo(variables.workspaceId, variables.branchName),
        refetchType: "all"
      });
      queryClient.invalidateQueries({
        queryKey: queryKeys.file.all(variables.workspaceId, variables.branchName),
        refetchType: "all"
      });
      // Pull does a `git fetch` under the hood — remote refs may have moved.
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.branches(variables.workspaceId)
      });
    }
  });
};

export const useFetchRemote = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ workspaceId, branchName }: { workspaceId: string; branchName: string }) =>
      WorkspaceService.fetchRemote(workspaceId, branchName),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.revisionInfo(variables.workspaceId, variables.branchName),
        refetchType: "all"
      });
      // Fetch can flip a branch's `origin` (e.g. `local_only` → `both`).
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.branches(variables.workspaceId)
      });
    }
  });
};

export const useDeleteBranch = (workspaceId: string) => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (branchName: string) => WorkspaceService.deleteBranch(workspaceId, branchName),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.branches(workspaceId)
      });
    }
  });
};

export const useForcePush = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ workspaceId, branchName }: { workspaceId: string; branchName: string }) =>
      WorkspaceService.forcePushBranch(workspaceId, branchName),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.revisionInfo(variables.workspaceId, variables.branchName)
      });
      // Force-push may have created the remote ref — refresh the badge.
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.branches(variables.workspaceId)
      });
    }
  });
};

export const useDiscardAllChanges = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ workspaceId, branchName }: { workspaceId: string; branchName: string }) =>
      WorkspaceService.discardAllChanges(workspaceId, branchName),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.revisionInfo(variables.workspaceId, variables.branchName)
      });
      queryClient.invalidateQueries({
        queryKey: queryKeys.file.all(variables.workspaceId, variables.branchName)
      });
    }
  });
};

export const usePushChanges = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      workspaceId,
      branchName,
      commitMessage
    }: {
      workspaceId: string;
      branchName: string;
      commitMessage?: string;
    }) => WorkspaceService.pushChanges(workspaceId, branchName, commitMessage),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.revisionInfo(variables.workspaceId, variables.branchName)
      });
      queryClient.invalidateQueries({
        queryKey: queryKeys.file.all(variables.workspaceId, variables.branchName)
      });
      // First push creates the remote ref, flipping `origin` from
      // `local_only` to `both` — refresh the badge.
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.branches(variables.workspaceId)
      });
    }
  });
};

/**
 * Fetches the workspace list for an org.
 *
 * Default reads the org id from the currentOrg Zustand store. Callers that
 * already know which org they're asking about (e.g. the dispatcher which
 * computes chosenOrg before the store is primed) should pass `orgIdOverride`
 * so the query key and fetched data can never drift out of sync with the
 * store while it catches up.
 */
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
