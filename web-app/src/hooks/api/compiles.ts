import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CompilesService, type RunCompileRequest } from "@/services/api/compiles";
import queryKeys from "./queryKey";

interface PollOptions {
  paused?: boolean;
  intervalMs?: number;
  /** Off entirely. The console home gates each source on the standing that decides
   *  whether its section renders, so a narrow staff grant never fires the request. */
  enabled?: boolean;
}

const DEFAULT_INTERVAL_MS = 5_000;

/**
 * Polls `GET /admin/compiles`. 5s default cadence — same as the lease
 * + internal-jobs pages — so the "started 3s ago" column on a
 * compiling row actually ticks.
 */
export const useCompiles = (
  params: { limit?: number; workspace_id?: string; org_id?: string; status?: string } = {},
  options: { paused?: boolean; intervalMs?: number } = {}
) => {
  const { paused = false, intervalMs = DEFAULT_INTERVAL_MS } = options;
  return useQuery({
    queryKey: queryKeys.compiles.list(params),
    queryFn: () => CompilesService.list(params),
    refetchInterval: paused ? false : intervalMs
  });
};

/**
 * Polls `GET /admin/compiles/workspaces` — the aggregated "By workspace"
 * rollup that backs the default view. Same 5s cadence as the flat list so
 * the LiveIndicator and "compiling" counts stay in lockstep.
 */
export const useCompileWorkspaces = (
  params: { limit?: number; offset?: number; q?: string; status?: string } = {},
  options: PollOptions = {}
) => {
  const { paused = false, intervalMs = DEFAULT_INTERVAL_MS, enabled = true } = options;
  return useQuery({
    queryKey: queryKeys.compiles.workspaces(params),
    queryFn: () => CompilesService.listWorkspaces(params),
    refetchInterval: paused ? false : intervalMs,
    enabled
  });
};

/**
 * Lazily fetches one workspace's revision history (`GET /admin/compiles
 * ?workspace_id=`) when its row is expanded. `enabled` gates the request
 * on `expanded` so collapsed rows never hit the network.
 */
export const useWorkspaceRevisions = (
  workspaceId: string,
  options: { enabled?: boolean; paused?: boolean; intervalMs?: number; limit?: number } = {}
) => {
  const { enabled = true, paused = false, intervalMs = DEFAULT_INTERVAL_MS, limit = 25 } = options;
  const params = { workspace_id: workspaceId, limit };
  return useQuery({
    queryKey: queryKeys.compiles.list(params),
    queryFn: () => CompilesService.list(params),
    enabled: enabled && Boolean(workspaceId),
    refetchInterval: paused ? false : intervalMs
  });
};

export const useCompileDetail = (
  revisionId: string | undefined,
  options: { paused?: boolean; intervalMs?: number } = {}
) => {
  const { paused = false, intervalMs = DEFAULT_INTERVAL_MS } = options;
  return useQuery({
    queryKey: queryKeys.compiles.detail(revisionId ?? ""),
    queryFn: () => CompilesService.detail(revisionId as string),
    enabled: Boolean(revisionId),
    refetchInterval: paused ? false : intervalMs
  });
};

export const useRunCompileNow = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (req: RunCompileRequest) => CompilesService.runNow(req),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.compiles.all });
    }
  });
};

export const useBackfillUncompiled = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => CompilesService.backfillUncompiled(),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.compiles.all });
    }
  });
};

export const usePromoteCompile = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (revisionId: string) => CompilesService.promote(revisionId),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.compiles.all });
    }
  });
};

export const useBatchRunCompiles = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (vars: { workspaceIds: string[]; promote: boolean }) =>
      CompilesService.batchRun(vars.workspaceIds, vars.promote),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.compiles.all });
    }
  });
};

export const useBatchPromoteCompiles = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (revisionIds: string[]) => CompilesService.batchPromote(revisionIds),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.compiles.all });
    }
  });
};
