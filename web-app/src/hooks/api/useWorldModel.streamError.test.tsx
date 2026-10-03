// @vitest-environment jsdom
import {
  focusManager,
  onlineManager,
  QueryClient,
  QueryClientProvider
} from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WorldModelService } from "@/services/api/worldModel";
import type { WmInstanceDetailEvent, WmMeasureBreakdownEvent } from "@/types/worldModel";
import { useWmFilterCounts, useWmInstanceDetail, useWmMeasureBreakdown } from "./useWorldModel";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "p1" }, branchName: "main" })
}));

vi.mock("@/services/api/worldModel", () => ({
  WorldModelService: {
    streamFilterCounts: vi.fn(),
    streamInstanceDetail: vi.fn(),
    streamMeasureBreakdown: vi.fn()
  }
}));

const streamFilterCounts = vi.mocked(WorldModelService.streamFilterCounts);
const streamInstanceDetail = vi.mocked(WorldModelService.streamInstanceDetail);
const streamMeasureBreakdown = vi.mocked(WorldModelService.streamMeasureBreakdown);

const failure = new Error("SSE connection failed with status: 500");

const init: WmMeasureBreakdownEvent = {
  kind: "init",
  root: "store.revenue",
  nodes: [
    {
      id: "store.revenue",
      view: "store",
      measure: "revenue",
      label: "Revenue",
      measure_type: "sum",
      is_composite: false,
      is_root: true,
      expr: null
    }
  ],
  edges: []
};

/** Every promise callback already queued has run by the time this resolves. */
const settled = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

/**
 * One query cache for the whole render, as the app has one for the whole page.
 * A wrapper that made a client of its own each time it rendered would start
 * every query again on a re-render.
 */
const sharedCache = () => {
  const client = new QueryClient();
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
};

beforeEach(() => {
  vi.clearAllMocks();
});

// Unmount every hook a test rendered. A hook left mounted keeps its query, which
// the next test's focus or reconnect would run again, and count as its own.
afterEach(() => {
  cleanup();
});

describe("useWmMeasureBreakdown", () => {
  const render = () =>
    renderHook(() => useWmMeasureBreakdown("store", "42", "revenue"), {
      wrapper: sharedCache()
    });

  it("surfaces a failed stream as the query's error, even after a partial tree arrived", async () => {
    streamMeasureBreakdown.mockImplementation((_p, _e, _k, _m, onEvent, _onClose, onError) => {
      queueMicrotask(() => {
        onEvent(init);
        onError(failure);
      });
    });

    const { result } = render();

    await waitFor(() => expect(result.current.error).toBe(failure));
    expect(result.current.isLoading).toBe(false);
  });

  it("surfaces a stream that closes before `done` as an error", async () => {
    streamMeasureBreakdown.mockImplementation((_p, _e, _k, _m, onEvent, onClose) => {
      queueMicrotask(() => {
        onEvent(init);
        onClose();
      });
    });

    const { result } = render();

    await waitFor(() => expect(result.current.error).toBeInstanceOf(Error));
  });

  it("resolves with the assembled tree when the stream finishes", async () => {
    streamMeasureBreakdown.mockImplementation((_p, _e, _k, _m, onEvent, onClose) => {
      queueMicrotask(() => {
        onEvent(init);
        onEvent({
          kind: "value",
          node_id: "store.revenue",
          value: "10",
          unvalued_reason: null
        });
        onEvent({ kind: "done" });
        onClose();
      });
    });

    const { result } = render();

    await waitFor(() => expect(result.current.data?.nodes[0].value).toBe("10"));
    expect(result.current.error).toBeNull();
  });

  /**
   * A finished tree, then `event` once the minute it is kept fresh for is over.
   * The stream used to run again on it, and its `init` set every value back to
   * empty while the warehouse queries ran again.
   */
  const afterTheTreeIsStale = async (event: () => void) => {
    streamMeasureBreakdown.mockImplementation((_p, _e, _k, _m, onEvent, onClose) => {
      queueMicrotask(() => {
        onEvent(init);
        onEvent({ kind: "value", node_id: "store.revenue", value: "10", unvalued_reason: null });
        onEvent({ kind: "done" });
        onClose();
      });
    });
    const { result } = render();
    await waitFor(() => expect(result.current.data?.nodes[0].value).toBe("10"));
    expect(streamMeasureBreakdown).toHaveBeenCalledTimes(1);

    const later = Date.now() + 61_000;
    vi.spyOn(Date, "now").mockReturnValue(later);
    try {
      act(event);
      await settled();
    } finally {
      vi.mocked(Date.now).mockRestore();
    }
    return result;
  };

  it("keeps the values it streamed when the window regains focus", async () => {
    try {
      const result = await afterTheTreeIsStale(() => {
        focusManager.setFocused(false);
        focusManager.setFocused(true);
      });
      expect(streamMeasureBreakdown).toHaveBeenCalledTimes(1);
      expect(result.current.data?.nodes[0].value).toBe("10");
    } finally {
      focusManager.setFocused(undefined);
    }
  });

  it("keeps the values it streamed when the network comes back", async () => {
    const result = await afterTheTreeIsStale(() => {
      onlineManager.setOnline(false);
      onlineManager.setOnline(true);
    });
    expect(streamMeasureBreakdown).toHaveBeenCalledTimes(1);
    expect(result.current.data?.nodes[0].value).toBe("10");
  });
});

describe("useWmFilterCounts", () => {
  it("reports a failed stream as an error and stops loading", async () => {
    streamFilterCounts.mockImplementation((_p, _e, _k, _onEvent, _onClose, onError) => {
      queueMicrotask(() => onError(failure));
    });

    const { result } = renderHook(() => useWmFilterCounts("store", "42"));

    await waitFor(() => expect(result.current.error).toBe(failure));
    expect(result.current.isLoading).toBe(false);
  });

  it("reports no error when the stream closes normally", async () => {
    streamFilterCounts.mockImplementation((_p, _e, _k, _onEvent, onClose) => {
      queueMicrotask(onClose);
    });

    const { result } = renderHook(() => useWmFilterCounts("store", "42"));

    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(result.current.error).toBeNull();
  });
});

describe("useWmInstanceDetail", () => {
  /** The page (measure chips on the cards) and the detail panel, on one instance. */
  const renderBothConsumers = () =>
    renderHook(
      () => ({
        page: useWmInstanceDetail("store", "42"),
        panel: useWmInstanceDetail("store", "42")
      }),
      { wrapper: sharedCache() }
    );

  const instance: WmInstanceDetailEvent = {
    kind: "init",
    entity_id: "store",
    key_value: "42",
    display: "Store 42",
    attributes: []
  };

  it("reports a failed stream as an error and stops loading", async () => {
    streamInstanceDetail.mockImplementation((_p, _e, _k, _onEvent, _onClose, onError) => {
      queueMicrotask(() => onError(failure));
    });

    const { result } = renderHook(() => useWmInstanceDetail("store", "42"), {
      wrapper: sharedCache()
    });

    await waitFor(() => expect(result.current.error).toBe(failure));
    expect(result.current.isLoading).toBe(false);
  });

  it("opens one stream for an instance, however many consumers read it", async () => {
    streamInstanceDetail.mockImplementation((_p, _e, _k, onEvent, onClose) => {
      queueMicrotask(() => {
        onEvent({
          kind: "measure_names",
          measure_names: [{ name: "revenue", measure_type: "sum" }]
        });
        onEvent(instance);
        onEvent({
          kind: "measure",
          computed_measures: [{ name: "revenue", measure_type: "sum", value: "10", fiber_count: 3 }]
        });
        onEvent({ kind: "done" });
        onClose();
      });
    });

    const { result } = renderBothConsumers();

    await waitFor(() => expect(result.current.panel.isLoading).toBe(false));
    expect(streamInstanceDetail).toHaveBeenCalledTimes(1);
    // Both read the one assembled instance, the page's measure values included.
    expect(result.current.page.data?.computed_measures[0].value).toBe("10");
    expect(result.current.page.data).toBe(result.current.panel.data);
    expect(result.current.page.error).toBeNull();
    expect(result.current.page.isLoading).toBe(false);
  });

  it("shows every consumer the same failure, and what arrived before it", async () => {
    streamInstanceDetail.mockImplementation((_p, _e, _k, onEvent, _onClose, onError) => {
      queueMicrotask(() => {
        onEvent(instance);
        onError(failure);
      });
    });

    const { result } = renderBothConsumers();

    await waitFor(() => expect(result.current.page.error).toBe(failure));
    expect(result.current.panel.error).toBe(failure);
    expect(streamInstanceDetail).toHaveBeenCalledTimes(1);
    expect(result.current.page.data?.display).toBe("Store 42");
    expect(result.current.panel.data?.display).toBe("Store 42");
    expect(result.current.panel.isLoading).toBe(false);
  });

  it("keeps loading until the stream is done, with what has arrived so far", async () => {
    let finish = () => {};
    streamInstanceDetail.mockImplementation((_p, _e, _k, onEvent, onClose) => {
      queueMicrotask(() => onEvent(instance));
      finish = () => {
        onEvent({ kind: "done" });
        onClose();
      };
    });

    const { result } = renderBothConsumers();

    await waitFor(() => expect(result.current.panel.data?.display).toBe("Store 42"));
    expect(result.current.panel.isLoading).toBe(true);
    expect(result.current.page.isLoading).toBe(true);

    finish();
    await waitFor(() => expect(result.current.panel.isLoading).toBe(false));
    expect(result.current.panel.error).toBeNull();
  });

  it("reports a stream that closes before `done` as an error", async () => {
    streamInstanceDetail.mockImplementation((_p, _e, _k, onEvent, onClose) => {
      queueMicrotask(() => {
        onEvent(instance);
        onClose();
      });
    });

    const { result } = renderBothConsumers();

    await waitFor(() => expect(result.current.panel.error).toBeInstanceOf(Error));
    expect(result.current.page.error).toBe(result.current.panel.error);
  });

  it("ends the stream of an instance nobody reads any more", async () => {
    // Streams that never finish: only an abort ends them.
    streamInstanceDetail.mockImplementation(() => {});

    const { rerender, unmount } = renderHook(
      ({ keyValue }: { keyValue: string }) => useWmInstanceDetail("store", keyValue),
      { initialProps: { keyValue: "42" }, wrapper: sharedCache() }
    );
    await waitFor(() => expect(streamInstanceDetail).toHaveBeenCalledTimes(1));
    const first = streamInstanceDetail.mock.calls[0][6];
    expect(first.aborted).toBe(false);

    rerender({ keyValue: "43" });
    await waitFor(() => expect(streamInstanceDetail).toHaveBeenCalledTimes(2));
    const second = streamInstanceDetail.mock.calls[1][6];
    expect(first.aborted).toBe(true);
    expect(second.aborted).toBe(false);

    unmount();
    expect(second.aborted).toBe(true);
  });

  it("opens no stream until an instance is chosen", () => {
    const { result } = renderHook(() => useWmInstanceDetail(null, null), {
      wrapper: sharedCache()
    });

    expect(streamInstanceDetail).not.toHaveBeenCalled();
    expect(result.current).toEqual({ data: null, isLoading: false, error: null });
  });
});
