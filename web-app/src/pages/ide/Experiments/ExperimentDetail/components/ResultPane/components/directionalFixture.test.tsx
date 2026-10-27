// @vitest-environment jsdom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ReportView, ResultResponse } from "@/types/experiment";
import ReportBody from "./ReportBody";

vi.mock("react-router-dom", () => ({ useNavigate: () => vi.fn() }));
vi.mock("@/hooks/api/useExperiments", () => ({
  useApplyWriteBack: () => ({ mutateAsync: vi.fn(), isPending: false })
}));
vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "ws-1" }, branchName: "main" })
}));
vi.mock("@/stores/useIdeBranch", () => ({ default: () => ({ setCurrentBranch: vi.fn() }) }));

afterEach(cleanup);

/**
 * The bytes `scripts/seed-experiment-report.sh` writes to `experiment_results.report`
 * and the agentic flow's second case reads. `vitest` runs from `web-app/`.
 */
const report = JSON.parse(
  readFileSync(
    resolve(process.cwd(), "tests/agentic/fixtures/experiments/directional-switchback-result.json"),
    "utf8"
  )
) as ReportView;

const result: ResultResponse = {
  experiment: report.experiment,
  run_id: "run-seeded",
  state: "reported",
  design_id: "design-seeded",
  aborted: false,
  created_at: "2026-05-04T02:00:00Z",
  report,
  effect: {},
  ratio: null,
  compliance: {
    c_treated: 1,
    c_control_leak: 0,
    n_treated: 6,
    n_control: 12,
    restated_mde: null,
    below_floor: false
  },
  write_back: { available: false, reason: null, edit: null },
  directional: report.directional,
  alpha: report.alpha,
  prediction: report.prediction,
  assumptions: report.assumptions
};

describe("the seeded directional switchback report", () => {
  it("reads as directional at the registered alpha, never as significant", () => {
    render(<ReportBody result={result} />);

    const verdict = screen.getByTestId("experiment-report-verdict");
    expect(verdict).toHaveTextContent("directional at α = 0.2");
    expect(verdict.textContent).not.toMatch(/significant/i);
    expect(screen.getByTestId("experiment-report-interval")).toHaveTextContent(
      "+$4.20/unit-day to +$14.60/unit-day"
    );
  });

  it("states the switchback's carryover and pull-forward assumptions", () => {
    render(<ReportBody result={result} />);

    const assumptions = screen.getByTestId("experiment-report-assumptions");
    expect(assumptions).toHaveTextContent("Carryover");
    expect(assumptions).toHaveTextContent("Pull-forward");
  });

  it("sets the frozen fitted slope against the measurement and agrees with it", () => {
    render(<ReportBody result={result} />);

    expect(screen.getByTestId("experiment-report-prior")).toHaveTextContent("fitted from history");
    const statement = screen.getByTestId("experiment-report-prediction-statement");
    expect(statement).toHaveAttribute("data-verdict", "agrees");
    expect(statement).toHaveTextContent("agrees with the World Model's slope");
  });

  it("projects onto no untreated units, because the whole fleet was treated", () => {
    render(<ReportBody result={result} />);

    expect(screen.queryByTestId("experiment-report-projection")).toBeNull();
    expect(screen.getByTestId("experiment-report-treated-total")).toBeInTheDocument();
  });

  it("shows no net figure", () => {
    const { container } = render(<ReportBody result={result} />);

    expect(container.textContent ?? "").not.toMatch(/\bnet\b/i);
  });
});
