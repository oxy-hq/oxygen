// @vitest-environment jsdom

import { fetchEventSource } from "@microsoft/fetch-event-source";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { EvalEventState, type TestStreamMessage } from "@/types/eval";
import useTestFileResults from "./useTestFileResults";

// `fetch-event-source` RESOLVES when its signal aborts — it does not reject with an
// `AbortError` — so a stopped case reaches the "stream closed" branch, not a catch. The
// stream below goes through the real library for that reason: a hand-rolled fake that
// rejected on abort would pin the wrong contract.

type RunTestCase = (
  onReadStream: (message: TestStreamMessage) => void,
  signal: AbortSignal
) => Promise<void>;

const { stream } = vi.hoisted(() => ({ stream: { run: undefined as RunTestCase | undefined } }));

vi.mock("@/services/api", () => ({
  TestFileService: {
    runTestCase: (
      _projectId: string,
      _branchName: string,
      _pathb64: string,
      _caseIndex: number,
      onReadStream: (message: TestStreamMessage) => void,
      _runIndex: number | undefined,
      signal: AbortSignal
    ) => stream.run?.(onReadStream, signal)
  }
}));

/** A request that stays open until its signal aborts, as a live SSE stream does. */
const openUntilAborted: typeof fetch = (_input, init) =>
  new Promise((_resolve, reject) => {
    init?.signal?.addEventListener("abort", () => {
      reject(new DOMException("The operation was aborted.", "AbortError"));
    });
  });

const liveStream: RunTestCase = (_onReadStream, signal) =>
  fetchEventSource("http://test/stream", { signal, openWhenHidden: true, fetch: openUntilAborted });

const ARGS = ["p1", "main", "tests/a.test.yml"] as const;
const caseState = () => useTestFileResults.getState().getCase(...ARGS, 0);
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  useTestFileResults.setState({ caseMap: new Map(), abortControllers: new Map() });
});

describe("useTestFileResults.runCase", () => {
  it("reports a stopped case as stopped, not as a run that ended without results", async () => {
    stream.run = liveStream;
    useTestFileResults.getState().runCase(...ARGS, 0);

    useTestFileResults.getState().stopFile(...ARGS);
    await flush();

    expect(caseState().error).toBe("Stopped by user");
    expect(useTestFileResults.getState().abortControllers.size).toBe(0);
  });

  it("reports a stream that closes on its own as a run without results", async () => {
    stream.run = () => Promise.resolve();
    useTestFileResults.getState().runCase(...ARGS, 0);
    await flush();

    expect(caseState().error).toBe("Run ended without results");
  });

  it("keeps a finished result when the file is stopped afterwards", async () => {
    const metric = { errors: [], metrics: [], stats: { total_attempted: 1, answered: 1 } };
    stream.run = (onReadStream, signal) => {
      onReadStream({ error: null, event: { type: EvalEventState.Finished, metric } });
      return liveStream(onReadStream, signal);
    };
    useTestFileResults.getState().runCase(...ARGS, 0);

    useTestFileResults.getState().stopFile(...ARGS);
    await flush();

    expect(caseState().error).toBeNull();
    expect(caseState().result).not.toBeNull();
  });

  it("keeps the error the stream reported when the file is stopped afterwards", async () => {
    stream.run = (onReadStream, signal) => {
      onReadStream({ error: "agent not found", event: null });
      return liveStream(onReadStream, signal);
    };
    useTestFileResults.getState().runCase(...ARGS, 0);

    useTestFileResults.getState().stopFile(...ARGS);
    await flush();

    expect(caseState().error).toBe("agent not found");
  });
  it("re-running a case stops the first run and leaves the second one stoppable", async () => {
    const signals: AbortSignal[] = [];
    stream.run = (onReadStream, signal) => {
      signals.push(signal);
      return liveStream(onReadStream, signal);
    };
    useTestFileResults.getState().runCase(...ARGS, 0);
    useTestFileResults.getState().runCase(...ARGS, 0);
    await flush();

    // The superseded stream is closed, and its ending wrote nothing over the new run.
    expect(signals[0].aborted).toBe(true);
    expect(signals[1].aborted).toBe(false);
    expect(caseState().error).toBeNull();
    // Stop still reaches the run that is actually in flight.
    expect(useTestFileResults.getState().abortControllers.size).toBe(1);
    useTestFileResults.getState().stopFile(...ARGS);
    await flush();
    expect(signals[1].aborted).toBe(true);
    expect(caseState().error).toBe("Stopped by user");
  });
});
