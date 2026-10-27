// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EstimateState, ExperimentListItem, ResultOutcome } from "@/types/experiment";
import ResultPane from "./index";

const h = vi.hoisted(() => ({ result: vi.fn(), run: vi.fn() }));

vi.mock("@/hooks/api/useExperiments", () => ({
  useExperimentResult: (...a: unknown[]) => h.result(...a),
  useExperimentRun: (...a: unknown[]) => h.run(...a),
  useApplyWriteBack: () => ({ mutateAsync: vi.fn(), isPending: false })
}));

const summary = { name: "beverage_promo_pilot", status: "aborted" } as ExperimentListItem;
const resolved = (data: unknown) => ({ data, isPending: false, error: null, refetch: vi.fn() });
const runWith = (estimate: EstimateState) => resolved({ run: { estimate } });

beforeEach(() => {
  h.result.mockReset();
  h.run.mockReset();
});
afterEach(cleanup);

describe("ResultPane — pending report", () => {
  it("shows a failed estimate's verbatim error and attempt count, not a silent spinner", () => {
    h.result.mockReturnValue(
      resolved({ kind: "pending", state: "aborted" } satisfies ResultOutcome)
    );
    h.run.mockReturnValue(
      runWith({
        state: "failed",
        task_status: "failed",
        error: "estimator crashed: `store_*` <b>x</b>",
        failed_at: "2026-03-10T00:00:00Z",
        attempts: 2
      })
    );
    render(<ResultPane summary={summary} />);

    expect(screen.getByTestId("experiment-report-pending")).toHaveTextContent("being prepared");
    expect(screen.getByTestId("experiment-report-pending-badge")).toHaveTextContent(
      "Estimate failed · attempt 2 of 3"
    );
    expect(screen.getByTestId("experiment-estimate")).toHaveTextContent("Attempt 2 of 3");
    expect(screen.getByTestId("experiment-estimate-error").textContent).toBe(
      "estimator crashed: `store_*` <b>x</b>"
    );
  });

  it("shows no estimate section while the estimate has not started", () => {
    h.result.mockReturnValue(
      resolved({ kind: "pending", state: "reported" } satisfies ResultOutcome)
    );
    h.run.mockReturnValue(runWith({ state: "none" }));
    render(<ResultPane summary={summary} />);

    expect(screen.getByTestId("experiment-report-pending")).toBeInTheDocument();
    expect(screen.getByTestId("experiment-report-pending-badge")).toHaveTextContent(
      "Preparing report"
    );
    expect(screen.queryByTestId("experiment-estimate")).toBeNull();
  });

  it("still shows the pending view when the run cannot be read", () => {
    h.result.mockReturnValue(
      resolved({ kind: "pending", state: "reported" } satisfies ResultOutcome)
    );
    h.run.mockReturnValue({
      data: undefined,
      isPending: false,
      error: new Error("x"),
      refetch: vi.fn()
    });
    render(<ResultPane summary={summary} />);

    expect(screen.getByTestId("experiment-report-pending")).toBeInTheDocument();
  });

  it("says what an aborted run did with its levers while its report is prepared", () => {
    h.result.mockReturnValue(
      resolved({ kind: "pending", state: "aborted" } satisfies ResultOutcome)
    );
    h.run.mockReturnValue(resolved({ run: { estimate: { state: "none" }, abort_levers: "keep" } }));
    render(<ResultPane summary={summary} />);

    expect(screen.getByTestId("experiment-abort-levers")).toHaveTextContent(
      "Levers were left as they were"
    );

    cleanup();
    h.run.mockReturnValue(resolved({ run: { estimate: { state: "none" }, abort_levers: "off" } }));
    render(<ResultPane summary={summary} />);
    expect(screen.getByTestId("experiment-abort-levers")).toHaveTextContent(
      "Levers were turned off"
    );
  });

  it("does not ask for the run unless the report is pending", () => {
    h.result.mockReturnValue(
      resolved({ kind: "sealed", state: "reported" } satisfies ResultOutcome)
    );
    render(<ResultPane summary={summary} />);

    expect(screen.getByTestId("experiment-sealed")).toBeInTheDocument();
    expect(h.run).not.toHaveBeenCalled();
  });
});
