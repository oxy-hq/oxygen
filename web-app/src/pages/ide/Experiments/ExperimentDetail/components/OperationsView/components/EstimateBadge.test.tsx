// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { EstimateState } from "@/types/experiment";
import EstimateBadge from "./EstimateBadge";

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

describe("EstimateBadge", () => {
  it("renders nothing before an estimate exists", () => {
    const { container } = render(<EstimateBadge estimate={{ state: "none" }} />);
    expect(container).toBeEmptyDOMElement();
  });

  it.each([
    [{ state: "running" }, "info", "Estimate running"],
    [{ state: "done" }, "success", "Estimate done"],
    [failed(), "danger", "Estimate failed"]
  ] as [EstimateState, string, string][])("tones %j as %s", (estimate, tone, text) => {
    render(<EstimateBadge estimate={estimate} />);
    expect(screen.getByTestId("experiment-estimate-state")).toHaveAttribute("data-tone", tone);
    expect(screen.getByTestId("experiment-estimate")).toHaveTextContent(text);
  });

  it("shows attempts and says it retries automatically, with no re-run control", () => {
    render(<EstimateBadge estimate={failed({ attempts: 2 })} />);
    expect(screen.getByTestId("experiment-estimate")).toHaveTextContent("Attempt 2 of 3");
    expect(screen.getByTestId("experiment-estimate")).toHaveTextContent("retries automatically");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("says it gave up once the attempts are spent", () => {
    render(<EstimateBadge estimate={failed({ attempts: 3 })} />);
    expect(screen.getByTestId("experiment-estimate")).toHaveTextContent("gave up after 3 attempts");
  });

  it("shows the server's error verbatim as text", () => {
    const error = "bad `query` <b>bold</b> *x* {{store_days.x}} & more";
    const { container } = render(<EstimateBadge estimate={failed({ error })} />);
    expect(screen.getByTestId("experiment-estimate-error").textContent).toBe(error);
    expect(container.querySelector("b")).toBeNull();
  });

  it("ignores an unknown estimate shape", () => {
    const { container } = render(<EstimateBadge estimate={41.7 as unknown as EstimateState} />);
    expect(container).toBeEmptyDOMElement();
  });
});
