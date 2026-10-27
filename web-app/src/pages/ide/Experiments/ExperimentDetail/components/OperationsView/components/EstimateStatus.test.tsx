// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { EstimateState } from "@/types/experiment";
import EstimateStatus from "./EstimateStatus";

afterEach(cleanup);

const failed = (
  over: Partial<Extract<EstimateState, { state: "failed" }>> = {}
): EstimateState => ({
  state: "failed",
  task_status: "failed",
  error: "estimator crashed",
  failed_at: "2026-03-10T00:00:00Z",
  attempts: 1,
  ...over
});

describe("EstimateStatus", () => {
  it("renders nothing before an estimate exists", () => {
    const { container } = render(<EstimateStatus estimate={{ state: "none" }} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("says the estimate is running", () => {
    render(<EstimateStatus estimate={{ state: "running" }} />);
    expect(screen.getByTestId("experiment-estimate")).toHaveTextContent("Estimate running");
  });

  it("says the estimate is done, without any effect number", () => {
    render(<EstimateStatus estimate={{ state: "done" }} />);
    const el = screen.getByTestId("experiment-estimate");
    expect(el).toHaveTextContent("Estimate done");
    expect(el.textContent).not.toMatch(/\d/);
  });

  it("shows a failure with its attempt count and says it retries automatically", () => {
    render(<EstimateStatus estimate={failed({ attempts: 2 })} />);
    const el = screen.getByTestId("experiment-estimate");
    expect(el).toHaveTextContent("Estimate failed");
    expect(el).toHaveTextContent("Attempt 2 of 3");
    expect(el).toHaveTextContent("retries automatically");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("says it gave up once the attempts are spent, and offers no re-run", () => {
    render(<EstimateStatus estimate={failed({ attempts: 3 })} />);
    const el = screen.getByTestId("experiment-estimate");
    expect(el).toHaveTextContent("gave up after 3 attempts");
    expect(el.textContent).not.toContain("retries automatically");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("shows the server's error verbatim as text, never markdown or html", () => {
    const error = "bad `query` <b>bold</b> *x* {{store_days.x}} & more";
    const { container } = render(<EstimateStatus estimate={failed({ error })} />);
    expect(screen.getByTestId("experiment-estimate-error").textContent).toBe(error);
    expect(container.querySelector("b")).toBeNull();
  });

  it("shows no error block when the server sent none", () => {
    render(<EstimateStatus estimate={failed({ error: null })} />);
    expect(screen.queryByTestId("experiment-estimate-error")).toBeNull();
  });
});
