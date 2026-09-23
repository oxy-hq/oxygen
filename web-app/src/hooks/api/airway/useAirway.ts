/**
 * Hooks for the `/agentic-airway` HTTP surface.
 *
 * Airway runs are queue-driven: `useStartAirwayRun` seeds the run, the
 * runtime coordinator drives it, and `useAirwayRunStream` consumes the
 * shared SSE endpoint and folds the event buffer through
 * `reduceAirwayEvents` into the phase-bar + resource-grid view model.
 *
 * Mirrors `useAgenticAutomations.ts`. Simpler: no snapshot endpoint, no
 * step DAG — the view is a pure function of the accumulated events,
 * and `reduceAirwayEvents` is idempotent over a prefix so we just
 * re-reduce the whole buffer on every tick.
 */

import {
  type UseMutationResult,
  type UseQueryResult,
  useMutation,
  useQuery,
  useQueryClient
} from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  type BackfillSuggestion,
  deriveBackfillSuggestion
} from "@/components/airway/backfillSuggestion";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import {
  type AirwayEvent,
  type AirwayRunSummary,
  AirwayService,
  type BackfillAirwayRequest,
  type BackfillRangeInfo,
  type ChunkedBackfillRequest,
  type ChunkedBackfillResponse,
  type CoverageReport,
  type DiscoveredTable,
  type DiscoverSourceRequest,
  type ResetCursorsOutcome,
  type ResetCursorsRequest,
  type StartAirwayRequest
} from "@/services/api/airway";
import { type AirwayRunView, reduceAirwayEvents } from "@/utils/airwayReducer";

import queryKeys from "../queryKey";

const keys = queryKeys.airway;

// ── Mutations ──────────────────────────────────────────────────────────────

export const useStartAirwayRun = (): UseMutationResult<
  { run_id: string },
  Error,
  StartAirwayRequest
> => {
  const { project } = useCurrentProjectBranch();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: StartAirwayRequest) => AirwayService.startRun(project.id, request),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: keys.runsForPipeline(project.id, variables.pipeline_ref)
      });
    }
  });
};

export const useBackfillAirway = (): UseMutationResult<
  { run_id: string },
  Error,
  BackfillAirwayRequest
> => {
  const { project } = useCurrentProjectBranch();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: BackfillAirwayRequest) => AirwayService.backfillRun(project.id, request),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: keys.runsForPipeline(project.id, variables.pipeline_ref)
      });
    }
  });
};

/**
 * Reset a pipeline's schema: drop its destination tables + clear the stored
 * schema/cursor so a later run re-infers from scratch. Destructive — the UI
 * gates it behind a confirm and nudges a backfill afterward. On success we
 * refresh run history + the ranges list so both reflect the cleared state.
 */
export const useResetSchema = (): UseMutationResult<
  { dropped_tables: string[] },
  Error,
  { pipeline_ref: string }
> => {
  const { project } = useCurrentProjectBranch();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ pipeline_ref }) => AirwayService.resetSchema(project.id, pipeline_ref),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: keys.runsForPipeline(project.id, variables.pipeline_ref)
      });
      queryClient.invalidateQueries({
        queryKey: keys.ranges(project.id, variables.pipeline_ref)
      });
      // A schema reset tombstones the state row, so the pipeline now holds no
      // cursors at all. Leaving this cached would offer a rewind picker listing
      // resources that no longer hold anything to rewind.
      queryClient.invalidateQueries({
        queryKey: keys.resourceCursors(project.id, variables.pipeline_ref)
      });
    }
  });
};

/**
 * The resources this pipeline holds a cursor for — the names {@link
 * useResetCursors} may be scoped to.
 *
 * `enabled` so the dialog fetches when it opens rather than on every render of
 * the pipeline page: the route resolves a `pipeline_ref` through the compile
 * boundary, which is not free, and nothing on screen needs the list until
 * someone is choosing from it.
 */
export const useAirwayResourceCursors = (
  pipelineRef: string,
  enabled: boolean
): UseQueryResult<string[], Error> => {
  const { project } = useCurrentProjectBranch();
  return useQuery({
    queryKey: keys.resourceCursors(project.id, pipelineRef),
    queryFn: () => AirwayService.resourceCursors(project.id, pipelineRef),
    enabled
  });
};

/**
 * Rewind a pipeline's cursors, keeping every landed row.
 *
 * The result type is a union, not a throw: a `409` refusal resolves as
 * `{ kind: "refused" }` so the caller renders the server's reasons instead of
 * an `AxiosError` message. Only 400/500/503 reject.
 *
 * Invalidation is on the cleared path only — a refusal changed nothing, and
 * invalidating after one would refetch the very list the operator is choosing
 * from while the refusal is still on screen.
 */
export const useResetCursors = (): UseMutationResult<
  ResetCursorsOutcome,
  Error,
  ResetCursorsRequest
> => {
  const { project } = useCurrentProjectBranch();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: ResetCursorsRequest) => AirwayService.resetCursors(project.id, request),
    onSuccess: (outcome, variables) => {
      if (outcome.kind !== "cleared") return;
      queryClient.invalidateQueries({
        queryKey: keys.resourceCursors(project.id, variables.pipeline_ref)
      });
    }
  });
};

/**
 * Start a resumable chunked backfill. The server drives the chunks detached;
 * on success we kick coverage polling + refresh run history so both reflect
 * the in-flight backfill. Re-invoking the same window resumes (skips `done`).
 */
export const useChunkedBackfill = (): UseMutationResult<
  ChunkedBackfillResponse,
  Error,
  ChunkedBackfillRequest
> => {
  const { project } = useCurrentProjectBranch();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: ChunkedBackfillRequest) =>
      AirwayService.chunkedBackfill(project.id, request),
    onSuccess: (data, variables) => {
      // Refresh the ranges list so the new range appears (newest first), its
      // own coverage, and run history.
      queryClient.invalidateQueries({
        queryKey: keys.ranges(project.id, variables.pipeline_ref)
      });
      queryClient.invalidateQueries({
        queryKey: keys.coverage(project.id, data.range_id)
      });
      queryClient.invalidateQueries({
        queryKey: keys.runsForPipeline(project.id, variables.pipeline_ref)
      });
    }
  });
};

/**
 * Resume a backfill range: re-run only its not-`done` chunks (the server reads
 * them from the range's checkpoints — no window needed). `pipeline_ref` is
 * carried only for cache invalidation. On success, refresh the range's coverage,
 * the ranges list, and run history.
 */
export const useResumeBackfill = (): UseMutationResult<
  ChunkedBackfillResponse,
  Error,
  { range_id: string; pipeline_ref: string }
> => {
  const { project } = useCurrentProjectBranch();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ range_id }) => AirwayService.resumeBackfill(project.id, { range_id }),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: keys.coverage(project.id, variables.range_id)
      });
      queryClient.invalidateQueries({
        queryKey: keys.ranges(project.id, variables.pipeline_ref)
      });
      queryClient.invalidateQueries({
        queryKey: keys.runsForPipeline(project.id, variables.pipeline_ref)
      });
    }
  });
};

const useCancelAirwayRun = (): UseMutationResult<void, Error, string> => {
  const { project } = useCurrentProjectBranch();
  return useMutation({
    mutationFn: (runId: string) => AirwayService.cancelRun(project.id, runId)
  });
};

/**
 * Connect to a source with live credentials and list its tables, for
 * the New Pipeline table picker. A mutation (not a query): it has an
 * outbound side effect and is keyed on user-entered credentials we
 * don't want to cache.
 */
export const useDiscoverSourceTables = (): UseMutationResult<
  DiscoveredTable[],
  Error,
  DiscoverSourceRequest
> => {
  const { project } = useCurrentProjectBranch();
  return useMutation({
    mutationFn: (request: DiscoverSourceRequest) =>
      AirwayService.discoverSourceTables(project.id, request)
  });
};

/** Run-history dropdown source — newest run first, capped at `limit`. */
export const useAirwayRuns = (
  pipelineRef: string,
  limit = 50
): UseQueryResult<AirwayRunSummary[]> => {
  const { project } = useCurrentProjectBranch();
  return useQuery({
    queryKey: keys.runsForPipeline(project.id, pipelineRef),
    queryFn: () => AirwayService.listRuns(project.id, pipelineRef, limit),
    enabled: !!pipelineRef
  });
};

/**
 * A pipeline's backfill ranges (newest first) with each range's chunk tally —
 * the source for the ranges gantt. Polls every 5s while any range is still
 * `running`, then goes quiet.
 */
export const useBackfillRanges = (pipelineRef: string): UseQueryResult<BackfillRangeInfo[]> => {
  const { project } = useCurrentProjectBranch();
  return useQuery({
    queryKey: keys.ranges(project.id, pipelineRef),
    queryFn: () => AirwayService.listBackfillRanges(project.id, pipelineRef),
    enabled: !!pipelineRef,
    refetchInterval: (query) => {
      const active = query.state.data?.some((r) => r.status === "running");
      return active ? 5000 : false;
    }
  });
};

/**
 * Per-range chunk coverage (status grid + rollup). Polls every 5s while any
 * chunk is still `pending`/`running`, so an in-flight range's progress streams
 * in, then goes quiet once everything is terminal. Disabled until a range is
 * selected.
 */
export const useAirwayCoverage = (rangeId: string | undefined): UseQueryResult<CoverageReport> => {
  const { project } = useCurrentProjectBranch();
  return useQuery({
    queryKey: keys.coverage(project.id, rangeId ?? ""),
    queryFn: () => AirwayService.coverage(project.id, rangeId as string),
    enabled: !!rangeId,
    refetchInterval: (query) => {
      const active = query.state.data?.chunks.some(
        (c) => c.status === "pending" || c.status === "running"
      );
      return active ? 5000 : false;
    }
  });
};

// ── Run stream ─────────────────────────────────────────────────────────────

/**
 * Where a subscription is in its life.
 *
 * Three states rather than a boolean because a consumer waiting for an event
 * has to tell "not subscribed yet" from "subscribed and finished" — collapsing
 * them is what leaves a placeholder on screen forever. `idle` is also the value
 * on the render *before* the effect subscribes, so a caller keyed off `settled`
 * never flashes a terminal message on the way to opening the stream.
 */
export type AirwayStreamStatus = "idle" | "open" | "closed";

export type AirwayRunStreamHandle = {
  /** Folded view model for the phase bar + resource grid. */
  view: AirwayRunView;
  /** Raw events in arrival order — backs the "Raw event trace" panel. */
  events: AirwayEvent[];
  /** True while the SSE connection is open. */
  streaming: boolean;
  /**
   * The stream reached an end — server close or a fatal transport error — so
   * whatever has not arrived by now is never arriving. `false` while idle:
   * "nothing to wait for" and "done waiting" are different answers.
   */
  settled: boolean;
  status: AirwayStreamStatus;
};

const IDLE_VIEW: AirwayRunView = reduceAirwayEvents([]);

/**
 * Subscribe to a run's SSE stream and expose the reduced view. Passing
 * `undefined` resets to the idle view (used before a run is launched).
 */
export const useAirwayRunStream = (runId: string | undefined): AirwayRunStreamHandle => {
  const { project } = useCurrentProjectBranch();
  const queryClient = useQueryClient();
  const [events, setEvents] = useState<AirwayEvent[]>([]);
  const [status, setStatus] = useState<AirwayStreamStatus>("idle");
  const abortRef = useRef<AbortController | null>(null);

  useEffect(() => {
    if (!runId) {
      setEvents([]);
      setStatus("idle");
      return;
    }
    const controller = new AbortController();
    abortRef.current = controller;
    setEvents([]);
    setStatus("open");

    AirwayService.streamEvents(project.id, runId, {
      signal: controller.signal,
      onEvent: (event) => {
        setEvents((prev) => [...prev, event]);
      },
      onError: (err) => {
        if (controller.signal.aborted) return;
        // Surface as a synthetic terminal error event so the reducer
        // drives the view to `failed` without a special code path.
        setEvents((prev) => [
          ...prev,
          {
            type: "pipeline_error",
            payload: { pipeline_name: "", load_id: null, error: err.message }
          }
        ]);
        // `fetchEventSource` treats a throw from its `onerror` as fatal and
        // does **not** then call `onclose`, so this is the only notice a
        // consumer gets that the stream is over. Without it, a connection
        // failure leaves every "still waiting" reader waiting forever.
        setStatus("closed");
      },
      onClose: () => {
        if (controller.signal.aborted) return;
        setStatus("closed");
        // The run-history list keys off `agentic_runs`; drop it so a
        // freshly-terminal run shows the right status on next read.
        queryClient.invalidateQueries({
          queryKey: [...keys.all, "runs-for-pipeline", project.id]
        });
      }
    }).catch(() => {
      // Connection-level failure already surfaced via `onError` (which also
      // closed the status); the `.catch` only stops an unhandled rejection.
      // Belt and braces so no rejection path can leave the status `open`.
      if (!controller.signal.aborted) setStatus("closed");
    });

    return () => {
      controller.abort();
      // Unsubscribed, not finished: a remount re-opens and re-reads.
      setStatus("idle");
    };
  }, [runId, project.id, queryClient]);

  const view = useMemo(
    () => (events.length === 0 ? IDLE_VIEW : reduceAirwayEvents(events)),
    [events]
  );

  return { view, events, streaming: status === "open", settled: status === "closed", status };
};

// ── Contract-derived backfill suggestion ───────────────────────────────────

export type BackfillSuggestionHandle = {
  suggestion: BackfillSuggestion;
  /** True while the contracts are still on their way — the caller shows a
   *  placeholder instead of the (momentarily indistinguishable) "nothing
   *  declared" message. **Terminates**: see the hook doc. */
  loading: boolean;
  /** The pipeline has never run, so no contract has ever been reported. */
  neverRan: boolean;
  /** The run history could not be read at all, so *whether* a contract exists
   *  is unknown. Distinct from `neverRan`, which is a fact about the pipeline
   *  rather than about this request. */
  runsError: boolean;
};

/**
 * The contracts a pipeline's resources were last pulled under, folded into a
 * suggested backfill window.
 *
 * There is no run-config API, so contracts are read the same way the Pipeline
 * Overview reads lineage: replay the latest run through the existing stream +
 * reducer — the whole event stream, for one `pipeline_plan` event. `enabled`
 * gates the SSE subscription, so the backfill modal only opens a stream while
 * it is on screen; the runs list underneath is a shared React Query key, so it
 * costs nothing extra. If run event volumes grow enough for that replay to be
 * felt, the fix is a run-config read endpoint, not a narrower replay.
 *
 * # `loading` is bounded by the stream's own end, not by a clock
 *
 * A latest run that failed *before* planning — admission refusal, a connector
 * build error, a malformed deployment row — replays to completion with zero
 * resource rows. Defining `loading` as "no resources yet" therefore never
 * cleared, and the modal sat on "Reading the source contracts…" for as long as
 * it stayed open: a placeholder that reads as *in progress* while it actually
 * means *never*.
 *
 * So it is bounded by [`AirwayRunStreamHandle.settled`], which covers all three
 * ways a subscription ends — the server closing the stream, a fatal transport
 * error, and unmount. A timer was the alternative and is worse in both
 * directions: short enough to help a dead stream is short enough to fire
 * mid-replay of a long run, which would report "no contract data" while the
 * events are still arriving — the same lie, told the other way round. The one
 * case a terminal branch does not bound is a socket that stays open and sends
 * nothing at all, and that is a broken SSE endpoint (every stream here must
 * emit a terminal event), not a state to design a timeout around.
 */
export const useBackfillSuggestion = (
  pipelineRef: string,
  enabled: boolean
): BackfillSuggestionHandle => {
  const { data: runs, isLoading, isError } = useAirwayRuns(pipelineRef);
  const latestRunId = runs?.[0]?.run_id;
  const { view, settled } = useAirwayRunStream(enabled ? latestRunId : undefined);
  const suggestion = useMemo(() => deriveBackfillSuggestion(view.resources), [view.resources]);
  return {
    suggestion,
    // `pipeline_plan` is the first event of a replay, so "resources are still
    // empty" is the honest definition of not-yet-known — but only until the
    // stream ends. Once it has, empty resources mean the run never reported a
    // contract, and the caller says so instead of waiting.
    loading: enabled && (isLoading || (!!latestRunId && !settled && view.resources.length === 0)),
    neverRan: !isLoading && !isError && !latestRunId,
    runsError: isError
  };
};

// ── Controller (launch + stream + stop) ────────────────────────────────────

/**
 * Ties the start mutation, the SSE stream, and cancel into one handle
 * for the run page. `launch` starts a run and begins streaming; `stop`
 * cancels it. The reduced `view` updates live.
 */
export const useAirwayRunController = () => {
  const start = useStartAirwayRun();
  const cancel = useCancelAirwayRun();
  const [runId, setRunId] = useState<string | undefined>(undefined);
  const stream = useAirwayRunStream(runId);

  const launch = useCallback(
    async (request: StartAirwayRequest) => {
      const { run_id } = await start.mutateAsync(request);
      setRunId(run_id);
      return run_id;
    },
    [start]
  );

  const stop = useCallback(async () => {
    if (!runId) return;
    await cancel.mutateAsync(runId);
  }, [cancel, runId]);

  return {
    runId,
    /** Adopt an existing run id (e.g. deep-link / reload). */
    setRunId,
    launch,
    stop,
    starting: start.isPending,
    stopping: cancel.isPending,
    ...stream
  };
};
