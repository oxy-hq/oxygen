// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AnalyticsStep, ArtifactItem } from "@/hooks/analyticsSteps";
import AnalyticsStepRow from "./AnalyticsStepRow";

const chart: ArtifactItem = {
  kind: "artifact",
  id: "tool-1",
  toolName: "render_chart",
  toolInput: JSON.stringify({ chart_type: "bar_chart" }),
  isStreaming: false
};

/** A finished step carrying every kind of control a row can hold. */
const STEP: AnalyticsStep = {
  kind: "step",
  id: "step-0",
  label: "Analyzing",
  isStreaming: false,
  llmUsage: { inputTokens: 10, outputTokens: 5, durationMs: 100, toolDurationMs: 0 },
  items: [
    { kind: "thinking", id: "think-0", text: "hmm", isStreaming: false },
    chart,
    {
      kind: "sql",
      id: "sql-2",
      sql: "SELECT 1",
      source: "semantic",
      is_preagg: true,
      isStreaming: false
    },
    {
      kind: "automation",
      id: "proc-run-3",
      automationName: "my_proc",
      steps: [{ name: "a", task_type: "execute_sql" }],
      stepsDone: 0,
      isStreaming: true
    },
    {
      kind: "builder_delegation",
      id: "delegation-4",
      childRunId: "child-1",
      request: "add a view",
      status: "running",
      isStreaming: true
    }
  ]
};

afterEach(cleanup);

describe("AnalyticsStepRow", () => {
  it("nests no interactive element inside another", () => {
    const { container } = render(
      <AnalyticsStepRow step={STEP} onSelectArtifact={vi.fn()} isSelected={() => false} />
    );
    // The row header, a pill per selectable item, and the rows and cards below.
    expect(container.querySelectorAll("button").length).toBeGreaterThan(5);
    const nested = container.querySelectorAll(
      "button button, button a[href], button input, button select, button textarea, button [tabindex]"
    );
    expect(nested).toHaveLength(0);
  });

  it("opens from its header, and picking a pill leaves it as it was", () => {
    const onSelect = vi.fn();
    render(<AnalyticsStepRow step={STEP} onSelectArtifact={onSelect} />);
    const header = screen.getByRole("button", { name: "Analyzing" });
    expect(header).toHaveAttribute("aria-expanded", "false");

    fireEvent.click(screen.getByTestId("reasoning-pill-render-chart"));
    expect(onSelect).toHaveBeenCalledWith(chart);
    expect(header).toHaveAttribute("aria-expanded", "false");

    fireEvent.click(header);
    expect(header).toHaveAttribute("aria-expanded", "true");
  });
});
