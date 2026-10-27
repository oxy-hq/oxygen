// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { ExperimentService } from "@/services/api/experiments";
import type { EstimateState, ResultOutcome, RunStatus } from "@/types/experiment";
import queryKeys from "./queryKey";
import { useExperimentDesign, useExperimentResult } from "./useExperiments";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "ws-1" }, branchName: "main" })
}));
vi.mock("@/services/api/experiments", () => ({
  ExperimentService: {
    getResult: vi.fn(async (): Promise<ResultOutcome> => ({ kind: "pending", state: "reported" })),
    getDesign: vi.fn(async () => ({ id: "d", content_hash: "h", frozen_at: null, config: {} }))
  }
}));

const runWith = (estimate: EstimateState) => ({ run: { estimate } }) as unknown as RunStatus;

/** The `refetchInterval` the result query is currently observed with, evaluated on its own state. */
const intervalFor = async (estimate: EstimateState | null) => {
  const client = new QueryClient();
  if (estimate) client.setQueryData(queryKeys.experiment.run("ws-1", "e"), runWith(estimate));
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  renderHook(() => useExperimentResult("e"), { wrapper });
  const query = client
    .getQueryCache()
    .find<ResultOutcome>({ queryKey: queryKeys.experiment.result("ws-1", "e") });
  await waitFor(() => expect(query?.state.data?.kind).toBe("pending"));
  const option = query?.observers[0]?.options.refetchInterval;
  return typeof option === "function" ? option(query as never) : option;
};

const failed = (attempts: number): EstimateState => ({
  state: "failed",
  task_status: "failed",
  error: "boom",
  failed_at: "2026-03-10T00:00:00Z",
  attempts
});

describe("useExperimentResult polling", () => {
  it("keeps polling a pending result while the estimate can still be retried", async () => {
    expect(await intervalFor(failed(2))).toBe(3_000);
    expect(await intervalFor({ state: "running" })).toBe(3_000);
    expect(await intervalFor(null)).toBe(3_000);
  });

  it("stops polling a pending result once the estimate has spent its attempts", async () => {
    expect(await intervalFor(failed(3))).toBe(false);
  });
});

describe("useExperimentDesign", () => {
  it("reads the design again when the experiment's status changes", async () => {
    const client = new QueryClient();
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const getDesign = vi.mocked(ExperimentService.getDesign);
    getDesign.mockClear();
    const { rerender } = renderHook(({ status }) => useExperimentDesign("e", status), {
      wrapper,
      initialProps: { status: "closing" }
    });
    await waitFor(() => expect(getDesign).toHaveBeenCalledTimes(1));
    // A refused analysis of past data lands: the design is now reopenable.
    rerender({ status: "reported" });
    await waitFor(() => expect(getDesign).toHaveBeenCalledTimes(2));
  });
});
