/**
 * Hooks for the new `/agentic-automations` HTTP surface.
 *
 * One file, one set of related hooks — small enough that splitting them
 * across files would be more friction than help. If this grows, split by
 * concern (files vs. runs vs. streaming).
 */

import {
  type UseMutationResult,
  type UseQueryResult,
  useMutation,
  useQuery,
  useQueryClient
} from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { parse as yamlParse } from "yaml";

import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import {
  type AutomationEvent,
  type AutomationFile,
  type AutomationRunSnapshot,
  type AutomationRunSummary,
  AutomationService,
  type StartAutomationRequest
} from "@/services/api/automations";
import queryKeys from "../queryKey";

const keys = queryKeys.agenticAutomation;

// ── Files ──────────────────────────────────────────────────────────────────

export const useAgenticAutomationFiles = (): UseQueryResult<AutomationFile[]> => {
  const { project } = useCurrentProjectBranch();
  return useQuery({
    queryKey: keys.files(project.id),
    queryFn: () => AutomationService.listFiles(project.id)
  });
};

/**
 * Parse the automation YAML for a given file and surface it in the
 * frontend `AutomationConfig` shape that components/automation/AutomationDiagram
 * already understands. The new `/agentic-automations/files` endpoint
 * returns raw text (no recursive sub-automation expansion the legacy
 * `useAutomationConfig` did) — we parse client-side so the diagram has
 * something to render. Sub-automation nodes appear as a single
 * placeholder; click-to-expand is a follow-up.
 */
export const useAgenticAutomationConfig = (
  pathB64: string
): UseQueryResult<AutomationConfigShape> => {
  const { project } = useCurrentProjectBranch();
  return useQuery({
    queryKey: [...keys.file(project.id, pathB64), "parsed"] as const,
    queryFn: async () => {
      const file = await AutomationService.getFile(project.id, pathB64);
      const parsed = (yamlParse(file.content) ?? {}) as Partial<AutomationConfigShape>;
      return {
        id: pathB64,
        path: file.path,
        name: parsed.name ?? file.path,
        tasks: parsed.tasks ?? [],
        variables: parsed.variables
      };
    },
    enabled: !!pathB64
  });
};

/**
 * Lightweight projection of the YAML — the legacy frontend
 * `AutomationConfig` is more strongly typed (discriminated union per
 * task), but the diagram only reads `tasks[].type` + `tasks[].name`,
 * so a permissive shape suffices and avoids dragging the full type
 * tree through the new module.
 */
export type AutomationConfigShape = {
  id: string;
  name: string;
  path: string;
  tasks: Array<Record<string, unknown> & { type?: string; name?: string }>;
  variables?: Record<string, unknown>;
};

// ── Start / cancel ─────────────────────────────────────────────────────────

const useStartAutomationRun = (): UseMutationResult<
  { run_id: string },
  Error,
  StartAutomationRequest
> => {
  const { project } = useCurrentProjectBranch();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: StartAutomationRequest) =>
      AutomationService.startRun(project.id, request),
    // The run-history dropdown reads from `runsForAutomation`. A new run won't
    // show up until the cache is dropped — without this, users see the old
    // list and the just-started run is missing.
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: keys.runsForAutomation(project.id, variables.workflow_ref)
      });
    }
  });
};

const useCancelAutomationRun = (): UseMutationResult<void, Error, string> => {
  const { project } = useCurrentProjectBranch();
  return useMutation({
    mutationFn: (runId: string) => AutomationService.cancelRun(project.id, runId)
  });
};

export const useAutomationRunSnapshot = (
  runId: string | undefined
): UseQueryResult<AutomationRunSnapshot> => {
  const { project } = useCurrentProjectBranch();
  return useQuery({
    queryKey: keys.run(project.id, runId ?? ""),
    queryFn: () => AutomationService.getRun(project.id, runId as string),
    enabled: !!runId
  });
};

/** History dropdown source — newest run first, capped at `limit` items. */
export const useAutomationRunsForAutomation = (
  automationRef: string | undefined,
  limit = 50
): UseQueryResult<AutomationRunSummary[]> => {
  const { project } = useCurrentProjectBranch();
  return useQuery({
    queryKey: keys.runsForAutomation(project.id, automationRef ?? ""),
    queryFn: () => AutomationService.listRuns(project.id, automationRef as string, limit),
    enabled: !!automationRef
  });
};

// ── Live run state (SSE + step-by-step status tracking) ────────────────────

type RunStepStatus = "pending" | "running" | "cached" | "success" | "failed" | "skipped";

/**
 * One iteration of a `loop_sequential` step's fan-out. Populated
 * live from `subrun_step_iteration_{started,completed}` events.
 * "running" entries arrive from `iteration_started`; the rest from
 * `iteration_completed`. Cache-hit iterations may also surface as
 * "done" without an intervening "running" because the decider
 * pre-seeds them at decide time (no started/completed pair fires
 * for those — the FE could fill them in from the snapshot's
 * `iterations` map, but for v1 we leave them implicit since
 * everything-cached short-circuits to step-level `cached`).
 */
export type LiveIteration = {
  index: number;
  value: unknown;
  status: "running" | "done" | "failed" | "cancelled";
  error?: string;
};

export type RunStepState = {
  name: string;
  taskType: string;
  status: RunStepStatus;
  /** Set when status === "cached" — the run that produced this output. */
  priorRunId?: string;
  errorMessage?: string;
  /**
   * Only populated for `loop_sequential` steps after the first
   * `subrun_step_iteration_started` arrives. The diagram's
   * `LoopProgressBar` reads from this; non-loop steps leave it
   * undefined.
   */
  iterations?: LiveIteration[];
};

type RunPhase = "idle" | "starting" | "running" | "completed" | "failed";

export type AutomationRunStream = {
  phase: RunPhase;
  steps: RunStepState[];
  /** Raw event log, oldest first. */
  events: AutomationEvent[];
  /** Set if the automation terminated abnormally. */
  error?: string;
};

export type AutomationRunStreamHandle = AutomationRunStream & {
  /**
   * Optimistically flip the local stream into a `failed` terminal phase
   * without waiting for the SSE `subrun_completed` event. Used by the
   * controller's `stop()` so the Stop button stops spinning the instant
   * the cancel API resolves — the backend's graceful cancellation may
   * still take seconds (in-flight SQL doesn't honor mid-step cancel),
   * during which the UI would otherwise keep showing the loading state.
   * Pending/running steps are coerced to `skipped`/`failed` mirroring
   * the `subrun_completed { success: false }` reducer branch.
   */
  markCancelled: () => void;
};

/**
 * Subscribes to `/agentic-automations/runs/:id/events` and reduces the event
 * stream into a per-step status map.
 *
 * Cache-hit events flip the step to `"cached"` (greyed-out dot in UI). The
 * stream auto-closes on `subrun_completed`.
 */
const useAutomationRunStream = (runId: string | undefined): AutomationRunStreamHandle => {
  const { project } = useCurrentProjectBranch();
  const queryClient = useQueryClient();
  const [stream, setStream] = useState<AutomationRunStream>({
    phase: runId ? "running" : "idle",
    steps: [],
    events: []
  });
  const abortRef = useRef<AbortController | null>(null);

  // Invalidate the run-history dropdown whenever this run reaches a
  // terminal phase. Without this the dot in `RunSelector` shows the
  // status from when the dropdown first loaded — typically `running`
  // — and never updates to `done`/`failed` after `subrun_completed`
  // arrives. Use a partial-match key (no automationRef) so we don't have
  // to thread it through this hook.
  useEffect(() => {
    if (stream.phase !== "completed" && stream.phase !== "failed") return;
    queryClient.invalidateQueries({
      queryKey: [...keys.all, "runs-for-automation", project.id]
    });
  }, [stream.phase, queryClient, project.id]);

  const markCancelled = useCallback(() => {
    setStream((prev) => {
      if (prev.phase === "completed" || prev.phase === "failed") {
        return prev;
      }
      const steps = prev.steps.map((s) => {
        if (s.status === "running") {
          return {
            ...s,
            status: "failed" as const,
            errorMessage: s.errorMessage ?? "Cancelled by user"
          };
        }
        if (s.status === "pending") {
          return {
            ...s,
            status: "skipped" as const,
            errorMessage: s.errorMessage ?? "Skipped — run cancelled"
          };
        }
        return s;
      });
      return {
        ...prev,
        phase: "failed",
        steps,
        error: prev.error ?? "Cancelled by user"
      };
    });
  }, []);

  useEffect(() => {
    if (!runId) {
      setStream({ phase: "idle", steps: [], events: [] });
      return;
    }
    const controller = new AbortController();
    abortRef.current = controller;
    setStream({ phase: "running", steps: [], events: [] });

    AutomationService.streamEvents(project.id, runId, {
      signal: controller.signal,
      onEvent: (event) => {
        setStream((prev) => reduceEvent(prev, event));
      },
      onError: (err) => {
        if (controller.signal.aborted) return;
        setStream((prev) => ({
          ...prev,
          phase: "failed",
          error: err.message
        }));
      },
      // The SSE backlog only replays events that were *persisted*. A run
      // that died before emitting `subrun_completed` (or one that we
      // force-failed in the DB to recover from a stranded coordinator)
      // has nothing closing the stream. Without this fallback the user
      // sees the run sit at `running` indefinitely even though the
      // server already considers it terminal. Reconcile with the
      // snapshot once the stream closes.
      onClose: () => {
        if (controller.signal.aborted) return;
        AutomationService.getRun(project.id, runId)
          .then((snapshot) => {
            if (controller.signal.aborted) return;
            setStream((prev) => reconcileWithSnapshot(prev, snapshot.status));
          })
          .catch(() => {
            // Snapshot fetch is best-effort; if it fails the user can
            // still refresh manually. No further error surfacing here —
            // the SSE error path covers connection failures.
          });
      }
    }).catch(() => {
      // Errors already surfaced via onError; the SSE helper rethrows.
    });

    return () => {
      controller.abort();
    };
  }, [project.id, runId]);

  return useMemo(() => ({ ...stream, markCancelled }), [stream, markCancelled]);
};

function reduceEvent(prev: AutomationRunStream, event: AutomationEvent): AutomationRunStream {
  const events = [...prev.events, event];

  switch (event.type) {
    case "subrun_started": {
      const steps: RunStepState[] = event.payload.steps.map((s) => ({
        name: s.name,
        taskType: s.task_type,
        status: "pending"
      }));
      return { ...prev, phase: "running", steps, events };
    }
    case "subrun_step_started": {
      const steps = prev.steps.map((s) =>
        s.name === event.payload.step ? { ...s, status: "running" as const } : s
      );
      return { ...prev, steps, events };
    }
    case "subrun_step_cache_hit": {
      // Cache-hit lands as a separate event before completed; pre-flip the
      // step so the dot appears immediately even if completed lags.
      const steps = prev.steps.map((s) =>
        s.name === event.payload.step
          ? {
              ...s,
              status: "cached" as const,
              priorRunId: event.payload.prior_run_id
            }
          : s
      );
      return { ...prev, steps, events };
    }
    case "subrun_step_completed": {
      const steps = prev.steps.map((s) => {
        if (s.name !== event.payload.step) return s;
        if (event.payload.success) {
          // Cache-hit completion preserves the cached dot; otherwise success.
          if (event.payload.cached || s.status === "cached") {
            return { ...s, status: "cached" as const };
          }
          return { ...s, status: "success" as const };
        }
        return {
          ...s,
          status: "failed" as const,
          errorMessage: event.payload.error
        };
      });
      return { ...prev, steps, events };
    }
    case "subrun_step_iteration_started": {
      const steps = prev.steps.map((s) => {
        if (s.name !== event.payload.step) return s;
        const existing = s.iterations ?? [];
        const idx = existing.findIndex((i) => i.index === event.payload.index);
        const newIter: LiveIteration = {
          index: event.payload.index,
          value: event.payload.value,
          status: "running"
        };
        // Upsert by index so a retry-replay of the same iteration
        // (force_invalidate flips a previously-done cell back to
        // running) overwrites cleanly. Sorted ascending so the
        // progress bar renders iterations in their canonical order.
        const iterations =
          idx === -1
            ? [...existing, newIter].sort((a, b) => a.index - b.index)
            : existing.map((i) => (i.index === newIter.index ? newIter : i));
        return { ...s, iterations };
      });
      return { ...prev, steps, events };
    }
    case "subrun_step_iteration_completed": {
      const steps = prev.steps.map((s) => {
        if (s.name !== event.payload.step) return s;
        const existing = s.iterations ?? [];
        // If completed lands before started — possible if the
        // backend ever batches them differently — synthesise a
        // running row first so the bar still has a cell to flip.
        const idx = existing.findIndex((i) => i.index === event.payload.index);
        const updated: LiveIteration =
          idx === -1
            ? {
                index: event.payload.index,
                value: undefined,
                status: event.payload.status,
                error: event.payload.error
              }
            : {
                ...existing[idx],
                status: event.payload.status,
                error: event.payload.error ?? existing[idx].error
              };
        const iterations =
          idx === -1
            ? [...existing, updated].sort((a, b) => a.index - b.index)
            : existing.map((i, k) => (k === idx ? updated : i));
        return { ...s, iterations };
      });
      return { ...prev, steps, events };
    }
    case "subrun_completed": {
      const success = event.payload.success;
      // When the automation terminates as failed, any step still flagged
      // `pending` or `running` is orphaned — usually because its
      // delegated outcome was lost (the per-run coordinator scoping bug)
      // or the run was force-failed in the DB. Coercing them to `failed`
      // here stops the diagram node spinner and the step list from
      // looping forever; on success we leave the steps alone because a
      // successful completion already implies all steps finished.
      // pending → `skipped` (never got its turn — downstream of the failure).
      // running → `failed` (was actively running when the automation halted, so
      //   most likely *is* the failure point or was killed mid-flight).
      const steps = success
        ? prev.steps
        : prev.steps.map((s) => {
            if (s.status === "running") {
              return {
                ...s,
                status: "failed" as const,
                errorMessage: s.errorMessage ?? "Automation halted while this step was running"
              };
            }
            if (s.status === "pending") {
              return {
                ...s,
                status: "skipped" as const,
                errorMessage:
                  s.errorMessage ?? "Skipped — an earlier step failed before reaching this one"
              };
            }
            return s;
          });
      return {
        ...prev,
        phase: success ? "completed" : "failed",
        steps,
        events
      };
    }
    default:
      return { ...prev, events };
  }
}

/**
 * Apply the run's persisted `task_status` to the reduced stream when the
 * SSE backlog didn't include a closing event. The snapshot field comes
 * from `agentic_runs.task_status`: `done`/`failed`/`cancelled`/`timed_out`
 * are terminal, anything else is treated as still-live.
 */
function reconcileWithSnapshot(prev: AutomationRunStream, status: string): AutomationRunStream {
  if (prev.phase === "completed" || prev.phase === "failed") {
    return prev;
  }
  const isTerminalSuccess = status === "done";
  const isTerminalFailure = status === "failed" || status === "cancelled" || status === "timed_out";
  if (!isTerminalSuccess && !isTerminalFailure) {
    return prev;
  }
  const steps = isTerminalSuccess
    ? prev.steps
    : prev.steps.map((s) => {
        if (s.status === "running") {
          return {
            ...s,
            status: "failed" as const,
            errorMessage: s.errorMessage ?? "Automation halted while this step was running"
          };
        }
        if (s.status === "pending") {
          return {
            ...s,
            status: "skipped" as const,
            errorMessage:
              s.errorMessage ?? "Skipped — an earlier step failed before reaching this one"
          };
        }
        return s;
      });
  return {
    ...prev,
    phase: isTerminalSuccess ? "completed" : "failed",
    steps
  };
}

// ── Combined run controller ────────────────────────────────────────────────

/**
 * Convenience hook that wires up start + stream + cancel for a single run.
 * Use directly from the run page; intermediate components don't need this.
 */
export const useAutomationRunController = () => {
  const queryClient = useQueryClient();
  const { project } = useCurrentProjectBranch();
  const start = useStartAutomationRun();
  const cancel = useCancelAutomationRun();
  const [runId, setRunId] = useState<string | undefined>(undefined);
  const stream = useAutomationRunStream(runId);

  const launch = useCallback(
    async (request: StartAutomationRequest) => {
      const { run_id } = await start.mutateAsync(request);
      setRunId(run_id);
      // Pre-warm the snapshot cache so a follow-up <Run page> render is fast.
      // `prefetchQuery` swallows a failed fetch (it resolves either way), so there
      // is nothing to catch — the run page's own query reports the error.
      void queryClient.prefetchQuery({
        queryKey: keys.run(project.id, run_id),
        queryFn: () => AutomationService.getRun(project.id, run_id)
      });
      return run_id;
    },
    [start, queryClient, project.id]
  );

  // `markCancelled` is `useCallback([])`-stable, so pulling it out keeps
  // `stop` from re-creating on every stream-state tick (which would in turn
  // re-render every component reading the controller).
  const markCancelled = stream.markCancelled;
  const stop = useCallback(async () => {
    if (!runId) return;
    try {
      await cancel.mutateAsync(runId);
    } finally {
      // Optimistically clear the loading state. The backend's graceful
      // cancellation winds down in the background, but an in-flight step
      // (e.g. a long SQL query) doesn't honor mid-execution cancel, so the
      // SSE `subrun_completed` event can lag the click by many seconds.
      // We flip the local stream into a terminal phase immediately so the
      // Stop button stops spinning the instant the API call resolves.
      markCancelled();
    }
  }, [cancel, runId, markCancelled]);

  /**
   * Re-run a single step (and everything downstream of it) by launching a
   * new run that points at the current run as its retry source and lists
   * the step name in `invalidate_steps`. The decider treats the named step
   * as a cache miss and the cascade handles the rest.
   *
   * Caller passes `automationRef` because the run page already has the
   * decoded path; the controller doesn't.
   */
  const replayStep = useCallback(
    async (automationRef: string, stepName: string) => {
      if (!runId) return;
      await launch({
        workflow_ref: automationRef,
        retry_from_run_id: runId,
        cache_enabled: true,
        invalidate_steps: [stepName]
      });
    },
    [launch, runId]
  );

  return useMemo(
    () => ({
      runId,
      setRunId,
      stream,
      launch,
      replayStep,
      stop,
      starting: start.isPending,
      cancelling: cancel.isPending
    }),
    [runId, stream, launch, replayStep, stop, start.isPending, cancel.isPending]
  );
};
