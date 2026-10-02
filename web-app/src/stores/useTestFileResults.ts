import { create } from "zustand";
import { TestFileService } from "@/services/api";
import { EvalEventState, type MetricValue, type RunStats } from "@/types/eval";

interface TestProgress {
  id: string | null;
  progress: number;
  total: number;
}

export interface TestCaseResult {
  errors: string[];
  metrics: MetricValue[];
  stats: RunStats;
}

export interface TestCaseState {
  state: EvalEventState | null;
  progress: TestProgress;
  error: string | null;
  result: TestCaseResult | null;
}

const defaultCaseState: TestCaseState = {
  state: null,
  progress: { id: null, progress: 0, total: 0 },
  error: null,
  result: null
};

interface TestFileResultsState {
  caseMap: Map<string, TestCaseState>;
  abortControllers: Map<string, AbortController>;
  setCase: (key: string, state: TestCaseState) => void;
  getCase: (projectId: string, branchName: string, pathb64: string, index: number) => TestCaseState;
  clearCasesForFile: (projectId: string, branchName: string, pathb64: string) => void;
  stopFile: (projectId: string, branchName: string, pathb64: string) => void;
  stopAll: () => void;
  runCase: (
    projectId: string,
    branchName: string,
    pathb64: string,
    index: number,
    runIndex?: number
  ) => void;
}

export const createCaseKey = (
  projectId: string,
  branchName: string,
  pathb64: string,
  index: number
): string => `${projectId}:${branchName}:${pathb64}:${index}`;

export const createCasePrefix = (projectId: string, branchName: string, pathb64: string): string =>
  `${projectId}:${branchName}:${pathb64}:`;

const useTestFileResults = create<TestFileResultsState>()((set, get) => ({
  caseMap: new Map(),
  abortControllers: new Map(),
  setCase: (key: string, state: TestCaseState) => {
    set((prev) => {
      const newMap = new Map(prev.caseMap);
      newMap.set(key, state);
      return { caseMap: newMap };
    });
  },
  getCase: (projectId, branchName, pathb64, index) => {
    const key = createCaseKey(projectId, branchName, pathb64, index);
    return get().caseMap.get(key) ?? { ...defaultCaseState };
  },
  clearCasesForFile: (projectId, branchName, pathb64) => {
    // Abort any active SSE streams before clearing to avoid zombie connections
    get().stopFile(projectId, branchName, pathb64);
    const prefix = createCasePrefix(projectId, branchName, pathb64);
    set((prev) => {
      const newMap = new Map(prev.caseMap);
      for (const key of newMap.keys()) {
        if (key.startsWith(prefix)) {
          newMap.delete(key);
        }
      }
      return { caseMap: newMap };
    });
  },
  stopFile: (projectId, branchName, pathb64) => {
    const prefix = createCasePrefix(projectId, branchName, pathb64);
    const { abortControllers } = get();
    for (const [key, controller] of abortControllers) {
      if (key.startsWith(prefix)) {
        controller.abort();
      }
    }
  },
  stopAll: () => {
    const { abortControllers } = get();
    for (const controller of abortControllers.values()) {
      controller.abort();
    }
  },
  runCase: (projectId, branchName, pathb64, index, runIndex?) => {
    const key = createCaseKey(projectId, branchName, pathb64, index);
    // A re-run replaces a run still in flight: close its stream, or both keep writing
    // to this case. `isCurrent` then keeps the superseded run's ending — which arrives
    // after this one has started — from overwriting this run's state or removing this
    // run's controller, which is what Stop needs to reach it.
    get().abortControllers.get(key)?.abort();
    get().setCase(key, { ...defaultCaseState });

    const abortController = new AbortController();
    const isCurrent = () => get().abortControllers.get(key) === abortController;
    set((prev) => {
      const newControllers = new Map(prev.abortControllers);
      newControllers.set(key, abortController);
      return { abortControllers: newControllers };
    });

    let receivedFinished = false;

    // Cannot reject past this chain: the `.catch` below turns every failure into the
    // case's error, and the `.then` / `.finally` after it only write store state.
    void TestFileService.runTestCase(
      projectId,
      branchName,
      pathb64,
      index,
      (message) => {
        if (!isCurrent()) return;
        const currentState = get().caseMap.get(key) ?? { ...defaultCaseState };
        const updated = { ...currentState };

        if (message.error) {
          updated.error = message.error;
          updated.state = null;
        } else if (message.event) {
          updated.state = message.event.type;
          switch (message.event.type) {
            case EvalEventState.Started:
              // Carries no payload: the `state` assignment above is the whole update.
              break;
            case EvalEventState.Progress:
              updated.progress = {
                id: message.event.id,
                progress: message.event.progress,
                total: message.event.total
              };
              break;
            case EvalEventState.Finished:
              receivedFinished = true;
              updated.result = {
                errors: message.event.metric.errors,
                metrics: message.event.metric.metrics,
                stats: message.event.metric.stats
              };
              break;
          }
        }
        get().setCase(key, updated);
      },
      runIndex,
      abortController.signal
    )
      .catch((err: Error) => {
        if (!isCurrent()) return;
        if (err.name === "AbortError") {
          get().setCase(key, {
            ...defaultCaseState,
            error: "Stopped by user"
          });
        } else {
          get().setCase(key, {
            ...defaultCaseState,
            error: err?.message ?? "Run failed"
          });
        }
      })
      .then(() => {
        // If the stream closed without sending a Finished event and no error was set,
        // the run must have failed silently — mark it as errored.
        // Note: state may still be null if the connection closed before any events arrived.
        //
        // A stop lands here, not in the catch above: `fetch-event-source` resolves when
        // its signal aborts rather than rejecting with an `AbortError`, so the signal is
        // what tells a stopped case from one whose stream closed on its own.
        const finalState = get().caseMap.get(key);
        if (isCurrent() && finalState && !receivedFinished && !finalState.error) {
          get().setCase(key, {
            ...finalState,
            state: null,
            error: abortController.signal.aborted ? "Stopped by user" : "Run ended without results"
          });
        }
      })
      .finally(() => {
        if (!isCurrent()) return;
        set((prev) => {
          const newControllers = new Map(prev.abortControllers);
          newControllers.delete(key);
          return { abortControllers: newControllers };
        });
      });
  }
}));

export default useTestFileResults;
