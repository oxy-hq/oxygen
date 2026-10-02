// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import fetchSSE from "./fetchSSE";
import { WorldModelService } from "./worldModel";

vi.mock("./fetchSSE", () => ({ default: vi.fn() }));

const fetchSSEMock = vi.mocked(fetchSSE);

type Callbacks = { onClose: () => void; onError: (error: Error) => void };

// Each stream, started with the same pair of callbacks, so one table covers all three.
const STREAMS: Record<string, (cb: Callbacks) => void> = {
  streamFilterCounts: ({ onClose, onError }) =>
    WorldModelService.streamFilterCounts(
      "p1",
      "store",
      "42",
      () => {},
      onClose,
      onError,
      new AbortController().signal
    ),
  streamInstanceDetail: ({ onClose, onError }) =>
    WorldModelService.streamInstanceDetail(
      "p1",
      "store",
      "42",
      () => {},
      onClose,
      onError,
      new AbortController().signal
    ),
  streamMeasureBreakdown: ({ onClose, onError }) =>
    WorldModelService.streamMeasureBreakdown(
      "p1",
      "store",
      "42",
      "revenue",
      () => {},
      onClose,
      onError,
      new AbortController().signal
    )
};

describe.each(Object.keys(STREAMS))("WorldModelService.%s", (name) => {
  beforeEach(() => {
    fetchSSEMock.mockReset();
  });

  it("reports a failed stream through onError, not as a close", async () => {
    const failure = new Error("SSE connection failed with status: 500");
    fetchSSEMock.mockRejectedValue(failure);
    const onClose = vi.fn();
    const onError = vi.fn();

    STREAMS[name]({ onClose, onError });

    await vi.waitFor(() => expect(onError).toHaveBeenCalledWith(failure));
    expect(onClose).not.toHaveBeenCalled();
  });

  it("reports a finished stream through onClose only", async () => {
    fetchSSEMock.mockImplementation(async (_url, options) => {
      options.onClose?.();
    });
    const onClose = vi.fn();
    const onError = vi.fn();

    STREAMS[name]({ onClose, onError });

    await vi.waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(onError).not.toHaveBeenCalled();
  });
});
