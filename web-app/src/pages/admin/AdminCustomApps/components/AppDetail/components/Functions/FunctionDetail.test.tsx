// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { AppFunctionSummary, FunctionInvocation, InvocationCalls } from "@/types/apps";
import { FunctionDetail } from "./FunctionDetail";

let invocations: FunctionInvocation[] = [];
const callsAsked: string[] = [];

// The hooks are the seam: this file is about how one row of history reads, not
// React Query's plumbing.
vi.mock("@/hooks/api/customApps/useAppFunctions", () => ({
  useFunctionInvocations: () => ({ isLoading: false, data: invocations }),
  useRunFunction: () => ({ mutate: vi.fn(), isPending: false }),
  useFunctionRun: () => ({ data: undefined }),
  useInvocationCalls: (_app: string, invocationId: string) => {
    callsAsked.push(invocationId);
    return { isPending: false, isError: false, data: TRACE };
  }
}));

const TRACE: InvocationCalls = {
  invocation_id: "iv-1",
  function: "sync-orders",
  environment: "production",
  build_id: "b-1",
  mode: "route",
  status: "success",
  failed: false,
  duration_ms: 120,
  created_at: "2026-09-20T10:00:00Z",
  recorded: true,
  reason: null,
  init_ms: 20,
  total_calls: 1,
  dropped: 0,
  unfinished: 0,
  retention_days: 3,
  calls: [
    {
      seq: 0,
      op: "query",
      family: "query",
      start_ms: 25,
      duration_ms: 80,
      status: "ok",
      verb: "SELECT",
      target: "orders",
      rows: 3
    }
  ]
};

afterEach(() => {
  cleanup();
  invocations = [];
  callsAsked.length = 0;
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
    expect(label.classList.contains(ADMIN_TONE.danger.text)).toBe(true);
    expect(label.classList.contains(ADMIN_TONE.ok.text)).toBe(false);
    expect(screen.queryByText("success")).toBeNull();
  });

  it("shows failed without a status when none was stored", () => {
    showRow(invocation({ failed: true, result_status: null }));
    expect(screen.getByText("failed").getAttribute("title")).toBe(HINT);
  });

  it("leaves a clean success green", () => {
    showRow(invocation({ failed: false }));
    const label = screen.getByText("success");
    expect(label.classList.contains(ADMIN_TONE.ok.text)).toBe(true);
    expect(label.hasAttribute("title")).toBe(false);
  });

  it("leaves an error as it was", () => {
    showRow(invocation({ status: "error", failed: true, error: "function threw" }));
    const label = screen.getByText("error");
    expect(label.classList.contains(ADMIN_TONE.danger.text)).toBe(true);
    expect(label.hasAttribute("title")).toBe(false);
  });

  it("mutes a cancel the platform did not count as a failure", () => {
    showRow(invocation({ status: "cancelled", failed: false }));
    const label = screen.getByText("cancelled");
    expect(label.classList.contains(ADMIN_TONE.muted.text)).toBe(true);
    expect(label.classList.contains(ADMIN_TONE.danger.text)).toBe(false);
  });
});

describe("invocation call waterfall", () => {
  it("opens the row's calls only when toggled, and asks for that invocation", () => {
    showRow(invocation({ id: "iv-1" }));
    const toggle = screen.getByTestId("admin-app-invocation-toggle");
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByTestId("admin-app-invocation-calls")).toBeNull();
    expect(callsAsked).toEqual([]);

    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByTestId("admin-app-invocation-calls")).toBeTruthy();
    expect(callsAsked).toContain("iv-1");
    const rows = screen.getAllByTestId("admin-app-invocation-calls-row");
    expect(rows.map((r) => r.getAttribute("data-op"))).toEqual(["query"]);

    fireEvent.click(toggle);
    expect(screen.queryByTestId("admin-app-invocation-calls")).toBeNull();
  });
});

describe("invocation error text", () => {
  // The 2026-10-21 outage: the cause sat past the truncation on every row.
  const OUTAGE =
    "function threw: HostError: ctx.oltp: could not open a transaction: connection error: " +
    "connect failed: error connecting to server\n    at oxy:bootstrap:154:19";

  it("opens the whole message with the row and closes it with the row", () => {
    showRow(invocation({ status: "error", failed: true, error: OUTAGE }));
    expect(screen.queryByTestId("admin-app-function-invocation-error-full")).toBeNull();

    const toggle = screen.getByTestId("admin-app-invocation-toggle");
    fireEvent.click(toggle);

    const full = screen.getByTestId("admin-app-function-invocation-error-full");
    // Whole, newlines and all — the stack is part of what an operator reads.
    expect(full.textContent).toBe(OUTAGE);
    // Beside the calls, not instead of them.
    expect(screen.getByTestId("admin-app-invocation-calls")).toBeTruthy();

    fireEvent.click(toggle);
    expect(screen.queryByTestId("admin-app-function-invocation-error-full")).toBeNull();
  });

  it("opens no error block on a row without an error", () => {
    showRow(invocation({ failed: false }));
    fireEvent.click(screen.getByTestId("admin-app-invocation-toggle"));
    expect(screen.getByTestId("admin-app-invocation-calls")).toBeTruthy();
    expect(screen.queryByTestId("admin-app-function-invocation-error-full")).toBeNull();
  });
});
