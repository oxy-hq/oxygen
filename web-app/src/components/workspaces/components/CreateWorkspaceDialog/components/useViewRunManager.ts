import { fetchEventSource } from "@microsoft/fetch-event-source";
import { useCallback, useEffect, useRef, useState } from "react";
import type { SseEvent } from "@/hooks/useAnalyticsRun";
import { previewRequestHeaders } from "@/libs/utils/preview";
import { AnalyticsService } from "@/services/api/analytics";

const MAX_CONCURRENT = 10;

interface ViewRunCallbacks {
  startViewRun: (table: string) => Promise<string>;
  onViewDone: (table: string) => void;
  onViewFailed: (table: string) => void;
}

export interface ViewRunTimingSnapshot {
  /** Number of completed view runs. */
  completed: number;
  /** Total number of view runs. */
  total: number;
  /** Average duration of completed runs in ms. */
  avgDurationMs: number;
  /** Estimated seconds remaining for all view runs to finish. */
  estimatedSecondsLeft: number;
}

/**
 * Manages parallel semantic view builder runs with concurrency cap.
 * Each run opens its own SSE connection, auto-accepts file_change
 * suspensions, and reports completion/failure via callbacks.
 */
export function useViewRunManager(
  projectId: string,
  callbacks: ViewRunCallbacks
): {
  /** Kick off parallel view runs for the given tables. */
  startAll: (tables: string[]) => void;
  /** Abort all in-flight view runs, best-effort cancel on the backend, and
   *  mark any still-running views as failed so the parent state reflects it. */
  cancelAll: () => void;
  /** All events collected from view runs (for artifact display). */
  events: SseEvent[];
  isRunning: boolean;
  /** Timing snapshot for time estimation (null until first view completes). */
  timing: ViewRunTimingSnapshot | null;
  /** Wall-clock elapsed ms since view runs started (for trace duration display). */
  elapsedMs: number;
} {
  const [events, setEvents] = useState<SseEvent[]>([]);
  const [isRunning, setIsRunning] = useState(false);
  const [timing, setTiming] = useState<ViewRunTimingSnapshot | null>(null);
  const [elapsedMs, setElapsedMs] = useState(0);

  // Tracks in-flight view runs: controller → { table, runId? }. The runId is
  // populated once startViewRun resolves, so cancelAll can fire the backend
  // cancel call; if the run hasn't created yet the abort alone is sufficient.
  interface InFlight {
    table: string;
    runId?: string;
  }
  const inFlight = useRef<Map<AbortController, InFlight>>(new Map());
  const callbacksRef = useRef(callbacks);
  callbacksRef.current = callbacks;
  const viewStartTimes = useRef<Map<string, number>>(new Map());
  const completedDurations = useRef<number[]>([]);
  const batchStartTime = useRef<number>(0);
  const elapsedTimer = useRef<ReturnType<typeof setInterval> | null>(null);
  // Signals to runSingleView that an abort is user-initiated so onclose is
  // treated as a cancellation (reject) rather than a graceful finish (resolve).
  const cancelledControllers = useRef<WeakSet<AbortController>>(new WeakSet());

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      for (const ctrl of inFlight.current.keys()) {
        ctrl.abort();
      }
      inFlight.current.clear();
      if (elapsedTimer.current) clearInterval(elapsedTimer.current);
    };
  }, []);

  const cancelAll = useCallback(() => {
    const entries = Array.from(inFlight.current.entries());
    inFlight.current.clear();
    for (const [ctrl, info] of entries) {
      cancelledControllers.current.add(ctrl);
      // Notify the parent first so setViewRunStatus("failed") dispatches before
      // the abort triggers any race with checkDone.
      try {
        callbacksRef.current.onViewFailed(info.table);
      } catch {
        // Callback errors shouldn't block the rest of the cancel.
      }
      ctrl.abort();
      if (info.runId) {
        AnalyticsService.cancelRun(projectId, info.runId).catch(() => {
          // Best-effort — the SSE is already closed; the backend will clean
          // up when it notices the connection is gone.
        });
      }
    }
    setIsRunning(false);
    if (elapsedTimer.current) {
      clearInterval(elapsedTimer.current);
      elapsedTimer.current = null;
    }
  }, [projectId]);

  const startAll = useCallback(
    (tables: string[]) => {
      if (tables.length === 0) return;
      setIsRunning(true);
      setEvents([]);
      setTiming(null);
      setElapsedMs(0);
      viewStartTimes.current.clear();
      completedDurations.current = [];
      batchStartTime.current = Date.now();
      // Tick elapsed time every second for the trace header
      if (elapsedTimer.current) clearInterval(elapsedTimer.current);
      elapsedTimer.current = setInterval(() => {
        setElapsedMs(Date.now() - batchStartTime.current);
      }, 1000);

      const queue = [...tables];
      let active = 0;
      let completed = 0;
      const total = tables.length;

      // Store events per table so that each run's events are contiguous
      // when flattened. This prevents interleaving from N parallel runs
      // which would confuse the sequential step builder in the reasoning trace.
      const perTableEvents = new Map<string, SseEvent[]>();

      const rebuildEvents = () => {
        const flat: SseEvent[] = [];
        for (const t of tables) {
          const evts = perTableEvents.get(t);
          if (evts) flat.push(...evts);
        }
        setEvents(flat);
      };

      const updateTiming = () => {
        const durations = completedDurations.current;
        if (durations.length === 0) return;
        const avg = durations.reduce((a, b) => a + b, 0) / durations.length;
        const remaining = total - completed;
        const queued = queue.length; // not yet dispatched
        const now = Date.now();

        // Time for in-flight views: estimate when the slowest active view will finish
        let maxInFlightRemaining = 0;
        for (const [, startTime] of viewStartTimes.current) {
          const elapsed = now - startTime;
          const expectedRemaining = avg - elapsed;
          if (expectedRemaining > maxInFlightRemaining) {
            maxInFlightRemaining = expectedRemaining;
          }
        }
        // Time for queued views: they'll start as in-flight ones complete
        const queuedBatches = queued > 0 ? Math.ceil(queued / Math.min(MAX_CONCURRENT, queued)) : 0;
        const queuedMs = queuedBatches * avg;

        const estimatedMs = Math.max(0, maxInFlightRemaining) + queuedMs;
        setTiming({
          completed,
          total,
          avgDurationMs: avg,
          estimatedSecondsLeft: remaining === 0 ? 0 : Math.max(1, Math.round(estimatedMs / 1000))
        });
      };

      const checkDone = () => {
        if (completed === total) {
          setIsRunning(false);
          // Freeze elapsed time at final value
          if (elapsedTimer.current) {
            clearInterval(elapsedTimer.current);
            elapsedTimer.current = null;
          }
          setElapsedMs(Date.now() - batchStartTime.current);
          setTiming((prev) =>
            prev ? { ...prev, completed: total, estimatedSecondsLeft: 0 } : null
          );
        }
      };

      const onViewComplete = (table: string) => {
        const startTime = viewStartTimes.current.get(table);
        if (startTime) {
          completedDurations.current.push(Date.now() - startTime);
          viewStartTimes.current.delete(table); // remove so it's not counted as in-flight
        }
      };

      const processNext = () => {
        if (active >= MAX_CONCURRENT) return;

        const table = queue.shift();
        if (table === undefined) return;
        active++;
        viewStartTimes.current.set(table, Date.now());

        const onTableEvents = (newEvents: SseEvent[]) => {
          const existing = perTableEvents.get(table) ?? [];
          existing.push(...newEvents);
          perTableEvents.set(table, existing);
          rebuildEvents();
        };

        runSingleView(
          projectId,
          table,
          callbacksRef.current,
          inFlight.current,
          cancelledControllers.current,
          onTableEvents
        )
          .then((outcome) => {
            active--;
            completed++;
            onViewComplete(table);
            if (outcome === "cancelled") {
              // onViewFailed was already invoked by cancelAll; don't double-notify.
            } else {
              callbacksRef.current.onViewDone(table);
            }
            updateTiming();
            checkDone();
            processNext();
          })
          .catch(() => {
            active--;
            completed++;
            onViewComplete(table);
            callbacksRef.current.onViewFailed(table);
            updateTiming();
            checkDone();
            processNext();
          });

        // Fill remaining concurrency slots
        processNext();
      };

      processNext();
    },
    [projectId]
  );

  return { startAll, cancelAll, events, isRunning, timing, elapsedMs };
}

/** Run a single view build: create run → open SSE → auto-accept → wait for done. */
async function runSingleView(
  projectId: string,
  table: string,
  callbacks: ViewRunCallbacks,
  inFlight: Map<AbortController, { table: string; runId?: string }>,
  cancelledControllers: WeakSet<AbortController>,
  onNewEvents: (events: SseEvent[]) => void
): Promise<"done" | "cancelled"> {
  const controller = new AbortController();
  // Register immediately so cancelAll can abort even if startViewRun is still
  // in flight (the create-run POST is itself cancellable via the same signal
  // path, and if it resolves after cancellation we still want to stop it).
  inFlight.set(controller, { table });

  let runId: string;
  try {
    runId = await callbacks.startViewRun(table);
  } catch (err) {
    inFlight.delete(controller);
    throw err;
  }
  if (controller.signal.aborted) {
    inFlight.delete(controller);
    return "cancelled";
  }
  inFlight.set(controller, { table, runId });

  return new Promise<"done" | "cancelled">((resolve, reject) => {
    let seqCounter = 0;
    let settled = false;
    const settle = (fn: () => void) => {
      settled = true;
      inFlight.delete(controller);
      controller.abort();
      fn();
    };

    const url = AnalyticsService.eventsUrl(projectId, runId);
    const token = localStorage.getItem("auth_token");

    fetchEventSource(url, {
      method: "GET",
      headers: { Authorization: token ?? "", ...previewRequestHeaders() },
      signal: controller.signal,
      openWhenHidden: true,

      onmessage(msg) {
        // SSE format: msg.event = event type, msg.data = JSON payload
        if (!msg.event) return;

        let parsed: Record<string, unknown> = {};
        try {
          parsed = JSON.parse(msg.data ?? "{}");
        } catch {
          return;
        }

        const event: SseEvent = {
          id: msg.id ?? String(seqCounter++),
          type: msg.event,
          data: parsed
        } as SseEvent;

        // Report all trace-relevant events (llm_usage, tool_call, file_changed, etc.)
        // Skip control events that aren't useful for the reasoning trace.
        if (msg.event !== "done" && msg.event !== "error" && msg.event !== "awaiting_input") {
          onNewEvents([event]);
        }

        if (msg.event === "done") {
          settle(() => resolve("done"));
        }

        if (msg.event === "error") {
          settle(() => reject(new Error("View run failed")));
        }

        if (msg.event === "cancelled") {
          settle(() => resolve("cancelled"));
        }

        // Handle suspensions: auto-accept file_change, treat fatal errors as failure
        if (msg.event === "awaiting_input") {
          const questions =
            (parsed as { questions?: Array<{ prompt: string; suggestions?: string[] }> })
              .questions ?? [];
          const isAutoAcceptable = questions.some((q) => {
            try {
              const p = JSON.parse(q.prompt);
              return p.type === "file_change";
            } catch {
              return q.suggestions?.some(
                (s) => s.toLowerCase() === "accept" || s.toLowerCase() === "reject"
              );
            }
          });
          if (isAutoAcceptable) {
            AnalyticsService.submitAnswer(projectId, runId, "Accept").catch(() => {});
          } else {
            settle(() => reject(new Error("View run suspended with unrecoverable error")));
          }
        }
      },

      onerror(err) {
        if (controller.signal.aborted) {
          throw err; // intentional abort, stop retrying
        }
        settle(() => reject(err));
        throw err; // stop retrying
      },

      onclose() {
        if (settled) return; // already resolved/rejected via done/error event
        // Distinguish a user-initiated abort (cancelAll) from a passive stream
        // close. The cancelAll path marks the controller in `cancelledControllers`
        // before aborting so we don't treat the cancel as a graceful completion.
        if (cancelledControllers.has(controller)) {
          settle(() => resolve("cancelled"));
          return;
        }
        // Stream closed without a terminal event — the run may have completed
        // before the SSE connection caught the "done" event (race condition).
        // Treat as success; the fallback artifact synthesis in the UI will handle it.
        settle(() => resolve("done"));
      }
    }).catch(() => {
      // fetchEventSource rejects when abort() is called — expected
    });
  });
}
