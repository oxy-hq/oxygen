import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import queryKeys from "@/hooks/api/queryKey";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { SimulationService } from "@/services/api/simulation";
import type { Policy, SimulationSpecInput } from "@/types/simulation";

/** The grid of declared worlds. Stable for a revision, so cache it hard. */
export function useSimulations() {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project?.id ?? "";

  return useQuery({
    queryKey: queryKeys.simulation.list(projectId, branchName),
    queryFn: () => SimulationService.listSimulations(projectId, branchName || undefined),
    enabled: Boolean(projectId),
    staleTime: 5 * 60 * 1000
  });
}

export function useSimulationRuns() {
  const { project } = useCurrentProjectBranch();
  const projectId = project?.id ?? "";

  return useQuery({
    queryKey: queryKeys.simulation.runs(projectId),
    queryFn: () => SimulationService.listRuns(projectId),
    enabled: Boolean(projectId)
  });
}

/**
 * One run, polled while it is still going.
 *
 * Polling rather than SSE, deliberately: periods are persisted as they land, so
 * the database is already the stream. An SSE channel would add a second source
 * of truth that has to be kept consistent with it — and the terminal-event rule
 * means every failure path would need to remember to close it. A run advances
 * once per period, which is seconds at best, so 2s is well inside the grain.
 */
export function useSimulationRun(runId: string | undefined) {
  const { project } = useCurrentProjectBranch();
  const projectId = project?.id ?? "";

  return useQuery({
    queryKey: queryKeys.simulation.run(projectId, runId ?? ""),
    queryFn: () => SimulationService.getRun(projectId, runId as string),
    enabled: Boolean(projectId && runId),
    // `queued` too, not just `running`: the row exists as soon as the run is
    // enqueued, before any worker has claimed it, and a run sitting there
    // still has to be polled through to `running` — otherwise the first fetch
    // lands on `queued`, this predicate reads false, and nothing ever asks
    // again.
    refetchInterval: (query) => {
      const status = query.state.data?.run.status;
      return status === "running" || status === "queued" ? 2000 : false;
    }
  });
}

/**
 * Checks a candidate world without writing anything — the form's "is this
 * coherent" gate, called right before the create/edit form persists.
 */
export function useValidateSimulationSpec() {
  const { project } = useCurrentProjectBranch();
  const projectId = project?.id ?? "";

  return useMutation({
    mutationFn: (spec: SimulationSpecInput) => SimulationService.validateSpec(projectId, spec)
  });
}

export function useStartSimulationRun() {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project?.id ?? "";
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ name, policies }: { name: string; policies?: Policy[] }) =>
      SimulationService.startRun(projectId, name, {
        policies,
        branchName: branchName || undefined
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.simulation.runs(projectId) });
    }
  });
}
