// @vitest-environment jsdom

import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { AutomationItem } from "@/hooks/analyticsSteps";

import AutomationDelegationCard from "./index";

function makeItem(overrides: Partial<AutomationItem> = {}): AutomationItem {
  return {
    kind: "automation",
    id: "proc-1",
    automationName: "my_automation",
    steps: [
      { name: "query_data", task_type: "execute_sql" },
      { name: "analyze", task_type: "execute_sql" },
      { name: "summarize", task_type: "formatter" }
    ],
    stepsDone: 0,
    isStreaming: true,
    ...overrides
  };
}

describe("AutomationDelegationCard", () => {
  it("renders automation name", () => {
    render(<AutomationDelegationCard item={makeItem()} onSelect={vi.fn()} />);
    expect(screen.getByText("my_automation")).toBeTruthy();
  });

  it("shows step count", () => {
    render(<AutomationDelegationCard item={makeItem({ stepsDone: 2 })} onSelect={vi.fn()} />);
    expect(screen.getByText("2/3 steps")).toBeTruthy();
  });

  it("shows progress bar with correct width", () => {
    const { container } = render(
      <AutomationDelegationCard item={makeItem({ stepsDone: 1 })} onSelect={vi.fn()} />
    );
    const fill = container.querySelector<HTMLElement>(
      "[data-testid='automation-delegation-card'] [data-testid='progress-fill']"
    );
    // 1/3 done = ~33.3%
    expect(fill?.style.width).toMatch(/33/);
  });

  it("calls onSelect when View button is clicked", () => {
    const handler = vi.fn();
    const item = makeItem();
    const { container } = render(<AutomationDelegationCard item={item} onSelect={handler} />);
    const btn = container.querySelector("[data-testid='view-details-button']") as HTMLElement;
    fireEvent.click(btn);
    expect(handler).toHaveBeenCalledWith(item);
  });

  it("says on View details whether the run's panel is open, when told", () => {
    const { container, rerender } = render(
      <AutomationDelegationCard item={makeItem()} onSelect={vi.fn()} pressed={true} />
    );
    const btn = within(container).getByTestId("view-details-button");
    expect(btn).toHaveAttribute("aria-pressed", "true");

    rerender(<AutomationDelegationCard item={makeItem()} onSelect={vi.fn()} pressed={false} />);
    expect(btn).toHaveAttribute("aria-pressed", "false");

    // A host that tracks no selection claims no state.
    rerender(<AutomationDelegationCard item={makeItem()} onSelect={vi.fn()} />);
    expect(btn).not.toHaveAttribute("aria-pressed");
  });

  it("shows spinner when streaming", () => {
    const { container } = render(
      <AutomationDelegationCard item={makeItem({ isStreaming: true })} onSelect={vi.fn()} />
    );
    // Loader2 has animate-spin class
    expect(container.querySelector(".animate-spin")).toBeTruthy();
  });

  it("shows checkmark when complete", () => {
    const { container } = render(
      <AutomationDelegationCard
        item={makeItem({ isStreaming: false, stepsDone: 3 })}
        onSelect={vi.fn()}
      />
    );
    // No spinner when not streaming
    expect(container.querySelector(".animate-spin")).toBeNull();
  });
});
