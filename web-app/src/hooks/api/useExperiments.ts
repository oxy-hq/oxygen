import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import queryKeys from "@/hooks/api/queryKey";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { ExperimentService } from "@/services/api/experiments";
import {
  type AbortRequest,
  type ExperimentDesignInput,
  MAX_ESTIMATE_ATTEMPTS,
  type PriceRequest,
  type Proposal,
  type RunStatus
} from "@/types/experiment";

const LIST_REFETCH_MS = 30_000;
const RUN_REFETCH_MS = 15_000;
const SEALED_REFETCH_MS = 10_000;
const PENDING_REFETCH_MS = 3_000;
const PROPOSAL_POLL_MS = 2_000;
const INTERIM_RUNNING_REFETCH_MS = 3_000;
const INTERIM_IDLE_REFETCH_MS = 60_000;

/** The project id only — never the branch (see `ExperimentService`). */
function useProjectId() {
  const { project } = useCurrentProjectBranch();
  return project?.id ?? "";
}

/** Every experiment and its status. Re-read so a status change shows up. */
export function useExperiments() {
  const projectId = useProjectId();
  return useQuery({
    queryKey: queryKeys.experiment.list(projectId),
    queryFn: () => ExperimentService.list(projectId),
    enabled: Boolean(projectId),
    refetchInterval: LIST_REFETCH_MS,
    // A 503 here means "could not look", which the page must show as an error
    // with a retry — not silently retry into an empty-looking list.
    retry: false
  });
}

/**
 * The proposal log. Polls only while `keepPolling` says a pricing request has a
 * candidate still in flight: this is a per-workspace read, not something to hammer on an idle page.
 */
export function useExperimentProposals(
  name: string,
  options: { keepPolling?: (log: Proposal[] | undefined) => boolean } = {}
) {
  const projectId = useProjectId();
  return useQuery({
    queryKey: queryKeys.experiment.proposals(projectId, name),
    queryFn: () => ExperimentService.getProposals(projectId, name),
    enabled: Boolean(projectId && name),
    refetchInterval: (query) =>
      options.keepPolling?.(query.state.data) ? PROPOSAL_POLL_MS : false,
    retry: false
  });
}

/** Enqueue pricing for 1 to 6 candidates. The returned `request_id` / `proposal_ids` are the rows to watch in the log. */
export function usePriceExperiment(name: string) {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: PriceRequest) => ExperimentService.price(projectId, name, request),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: queryKeys.experiment.proposals(projectId, name) })
  });
}

/** Operations view. Mounted only for registered / live / closing. */
export function useExperimentRun(name: string) {
  const projectId = useProjectId();
  return useQuery({
    queryKey: queryKeys.experiment.run(projectId, name),
    queryFn: () => ExperimentService.getRun(projectId, name),
    enabled: Boolean(projectId && name),
    refetchInterval: RUN_REFETCH_MS,
    retry: false
  });
}

/** Abort with a recorded reason and lever choice, then refresh everything that shows the status. */
export function useAbortExperiment(name: string) {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: AbortRequest) => ExperimentService.abort(projectId, name, request),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.experiment.all })
  });
}

/**
 * The interim reading of a live / closing run. Polled every 3s while a reading is
 * being computed, otherwise once a minute (the server also reads one a day).
 */
export function useExperimentInterim(name: string, enabled = true) {
  const projectId = useProjectId();
  return useQuery({
    queryKey: queryKeys.experiment.interim(projectId, name),
    queryFn: () => ExperimentService.getInterim(projectId, name),
    enabled: enabled && Boolean(projectId && name),
    refetchInterval: (query) => {
      const data = query.state.data;
      return data?.kind === "interim" && data.interim.running
        ? INTERIM_RUNNING_REFETCH_MS
        : INTERIM_IDLE_REFETCH_MS;
    },
    refetchOnWindowFocus: false,
    retry: false
  });
}

/** Ask for a fresh interim reading; the interim query is re-read on success. */
export function useRequestInterim(name: string) {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => ExperimentService.requestInterim(projectId, name),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: queryKeys.experiment.interim(projectId, name) })
  });
}

/**
 * The result, `sealed`, or `pending`.
 *
 * Mounted only for reported / aborted (`ResultPane`) — that mounting is what
 * keeps the UI from ever asking for an effect early. Both 409 answers mean the
 * report is not released yet, so they are re-read until it lands (a pending row
 * is being written, so sooner); a report is immutable and is not refetched on focus.
 * A pending report whose estimate has spent all its attempts will never land by
 * itself, so polling stops (the run query, which the pending view mounts, holds
 * the estimate; reading its cache keeps this hook's signature).
 */
export function useExperimentResult(name: string) {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  return useQuery({
    queryKey: queryKeys.experiment.result(projectId, name),
    queryFn: () => ExperimentService.getResult(projectId, name),
    enabled: Boolean(projectId && name),
    refetchInterval: (query) => {
      const kind = query.state.data?.kind;
      if (kind === "pending") {
        const run = queryClient.getQueryData<RunStatus>(queryKeys.experiment.run(projectId, name));
        const estimate = run?.run?.estimate;
        const exhausted =
          estimate?.state === "failed" && estimate.attempts >= MAX_ESTIMATE_ATTEMPTS;
        return exhausted ? false : PENDING_REFETCH_MS;
      }
      return kind === "sealed" ? SEALED_REFETCH_MS : false;
    },
    refetchOnWindowFocus: false,
    retry: false
  });
}

/** Create the review branch that carries the coefficient patch. */
export function useApplyWriteBack(name: string) {
  const projectId = useProjectId();
  return useMutation({
    mutationFn: () => ExperimentService.applyWriteBack(projectId, name)
  });
}

/** The experiment's one design (draft or frozen). */
export function useExperimentDesign(name: string, status?: string) {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  // What the design allows follows the run: a refused analysis of past data makes a
  // frozen design reopenable, so read it again whenever the status moves.
  useEffect(() => {
    if (status === undefined || !projectId) return;
    queryClient.invalidateQueries({ queryKey: queryKeys.experiment.design(projectId, name) });
  }, [status, projectId, name, queryClient]);
  return useQuery({
    queryKey: queryKeys.experiment.design(projectId, name),
    queryFn: () => ExperimentService.getDesign(projectId, name),
    enabled: Boolean(projectId && name),
    retry: false
  });
}

/** Create an experiment (a draft design), then refresh the list. */
export function useCreateExperiment() {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (config: ExperimentDesignInput) => ExperimentService.create(projectId, config),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.experiment.all })
  });
}

/** Save the draft in place over `baseHash`; refreshes the design, proposals (stale flags) and list. */
export function useSaveDesign(name: string) {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: { config: ExperimentDesignInput; baseHash?: string }) =>
      ExperimentService.saveDesign(projectId, name, input.config, input.baseHash),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.experiment.all })
  });
}

/** Copy this experiment's design as a new draft under `newName`; refreshes the list. */
export function useDuplicateExperiment(name: string) {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (newName: string) => ExperimentService.duplicate(projectId, name, newName),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.experiment.all })
  });
}

/** Adopt a done proposal into the draft. */
export function useUseProposal(name: string) {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (proposalId: string) =>
      ExperimentService.adoptProposal(projectId, name, proposalId),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.experiment.all })
  });
}

/** Register (freeze) the design, irreversibly; refreshes list, run and design. */
export function useRegisterExperiment(name: string) {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (contentHash: string) => ExperimentService.register(projectId, name, contentHash),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.experiment.all })
  });
}

/** Analyse a past-dated design from existing data; refreshes list, run and design. */
export function useRetrospectiveExperiment(name: string) {
  const projectId = useProjectId();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (contentHash: string) =>
      ExperimentService.retrospective(projectId, name, contentHash),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.experiment.all })
  });
}
