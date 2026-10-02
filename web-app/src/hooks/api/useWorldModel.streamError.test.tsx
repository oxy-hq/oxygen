// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { WorldModelService } from "@/services/api/worldModel";
import type { WmMeasureBreakdownEvent } from "@/types/worldModel";
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

const withQueryClient = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient()}>{children}</QueryClientProvider>
);

beforeEach(() => {
  vi.clearAllMocks();
});

describe("useWmMeasureBreakdown", () => {
  const render = () =>
    renderHook(() => useWmMeasureBreakdown("store", "42", "revenue"), {
      wrapper: withQueryClient
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
  it("reports a failed stream as an error and stops loading", async () => {
    streamInstanceDetail.mockImplementation((_p, _e, _k, _onEvent, _onClose, onError) => {
      queueMicrotask(() => onError(failure));
    });

    const { result } = renderHook(() => useWmInstanceDetail("store", "42"));

    await waitFor(() => expect(result.current.error).toBe(failure));
    expect(result.current.isLoading).toBe(false);
  });
});
