// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppFunctionSummary, FunctionInvocation } from "@/types/apps";
import { FunctionDetail } from "./FunctionDetail";

let invocations: FunctionInvocation[] = [];

// The hooks are the seam: this file is about how one row of history reads, not
// React Query's plumbing.
vi.mock("@/hooks/api/customApps/useAppFunctions", () => ({
  useFunctionInvocations: () => ({ isLoading: false, data: invocations }),
  useRunFunction: () => ({ mutate: vi.fn(), isPending: false }),
  useFunctionRun: () => ({ data: undefined })
}));

afterEach(() => {
  cleanup();
  invocations = [];
});

const FN: AppFunctionSummary = {
  name: "sync-orders",
  route: true,
  schedule: null,
  timezone: null,
  airway: false,
  timeout_seconds: null,
  retries: null,
  secrets_write: false,
  destinations: [],
  input_example: null
};

const invocation = (over: Partial<FunctionInvocation>): FunctionInvocation => ({
  id: "iv-1",
  mode: "route",
  status: "success",
  failed: false,
  result_status: null,
  duration_ms: 120,
  error: null,
  created_at: "2026-09-20T10:00:00Z",
  has_result: false,
  ...over
});

const HINT =
  "Returned without throwing, but the platform counted it as a failure — it answered 5xx or caught a failed ctx call.";

const showRow = (iv: FunctionInvocation) => {
  invocations = [iv];
  render(<FunctionDetail appId='app-1' fn={FN} />);
};

describe("invocation history status", () => {
  // The Sep 2026 warehouse incident: a function that caught every refused
  // write and answered its own 500 was recorded `success`, and a week of
  // history read green.
  it("shows a success the platform counted as a failure as failed, with its status", () => {
    showRow(invocation({ failed: true, result_status: 500 }));
    const label = screen.getByText("failed · 500");
    expect(label.getAttribute("title")).toBe(HINT);
    expect(label.classList.contains("text-destructive")).toBe(true);
    expect(label.classList.contains("text-success")).toBe(false);
    expect(screen.queryByText("success")).toBeNull();
  });

  it("shows failed without a status when none was stored", () => {
    showRow(invocation({ failed: true, result_status: null }));
    expect(screen.getByText("failed").getAttribute("title")).toBe(HINT);
  });

  it("leaves a clean success green", () => {
    showRow(invocation({ failed: false }));
    const label = screen.getByText("success");
    expect(label.classList.contains("text-success")).toBe(true);
    expect(label.hasAttribute("title")).toBe(false);
  });

  it("leaves an error as it was", () => {
    showRow(invocation({ status: "error", failed: true, error: "function threw" }));
    const label = screen.getByText("error");
    expect(label.classList.contains("text-destructive")).toBe(true);
    expect(label.hasAttribute("title")).toBe(false);
  });
});
