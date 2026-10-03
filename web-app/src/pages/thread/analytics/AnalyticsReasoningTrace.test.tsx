// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { UiBlock } from "@/services/api/analytics";
import AnalyticsReasoningTrace from "./AnalyticsReasoningTrace";

// Radix UI Tooltip requires browser APIs not present in jsdom
vi.mock("@/components/ui/shadcn/tooltip", () => ({
  Tooltip: ({ children }: { children: ReactNode }) => <>{children}</>,
  TooltipContent: () => null,
  TooltipTrigger: ({ children }: { children: ReactNode }) => <>{children}</>
}));

// ── helpers ───────────────────────────────────────────────────────────────────

let seq = 0;
const ev = <T extends UiBlock["event_type"]>(
  type: T,
  payload: Extract<UiBlock, { event_type: T }>["payload"]
): UiBlock => ({ seq: seq++, event_type: type, payload }) as UiBlock;

// Events emitted inside a fan-out card carry that card's `sub_spec_index`; the
// reducer routes on it, so a step without one lands outside every card.
const stepStart = (label: string, subSpecIndex?: number) =>
  ev("step_start", { label, ...(subSpecIndex != null ? { sub_spec_index: subSpecIndex } : {}) });
const stepEnd = (subSpecIndex?: number) =>
  ev("step_end", {
    label: "",
    outcome: "advanced",
    ...(subSpecIndex != null ? { sub_spec_index: subSpecIndex } : {})
  });
const fanOutStart = (total: number) => ev("fan_out_start", { total });
const subSpecStart = (index: number, label: string) =>
  ev("sub_spec_start", { index, total: 0, label });
const subSpecEnd = (index: number) => ev("sub_spec_end", { index, success: true });
const fanOutEnd = () => ev("fan_out_end", { success: true });

const noop = () => {};

// ── setup / teardown ──────────────────────────────────────────────────────────

beforeEach(() => {
  seq = 0;
});
afterEach(() => {
  cleanup();
});

// ── basic rendering ───────────────────────────────────────────────────────────

describe("AnalyticsReasoningTrace", () => {
  it("renders nothing when not running and no events", () => {
    const { container } = render(
      <AnalyticsReasoningTrace events={[]} isRunning={false} onSelectArtifact={noop} />
    );
    expect(container.firstChild).toBeNull();
  });

  it("renders a step label", () => {
    render(
      <AnalyticsReasoningTrace
        events={[stepStart("Analyzing"), stepEnd()]}
        isRunning={false}
        onSelectArtifact={noop}
      />
    );
    expect(screen.getByText("Analyzing")).toBeInTheDocument();
  });

  it("shows Reasoning trace header", () => {
    render(
      <AnalyticsReasoningTrace
        events={[stepStart("Solving"), stepEnd()]}
        isRunning={false}
        onSelectArtifact={noop}
      />
    );
    expect(screen.getByText("Reasoning trace")).toBeInTheDocument();
  });

  // ── fan-out card steps ──────────────────────────────────────────────────────

  it("renders step labels inside a fan-out card", () => {
    render(
      <AnalyticsReasoningTrace
        events={[
          fanOutStart(1),
          subSpecStart(0, "Q1"),
          stepStart("Solving", 0),
          stepEnd(0),
          subSpecEnd(0),
          fanOutEnd()
        ]}
        isRunning={false}
        onSelectArtifact={noop}
      />
    );
    expect(screen.getByText("Solving")).toBeInTheDocument();
  });

  it("first card is shown, second card is not yet in the DOM", () => {
    render(
      <AnalyticsReasoningTrace
        events={[
          fanOutStart(2),
          subSpecStart(0, "Q1"),
          stepStart("Solving for Q1", 0),
          stepEnd(0),
          subSpecEnd(0),
          subSpecStart(1, "Q2"),
          stepStart("Solving for Q2", 1),
          stepEnd(1),
          subSpecEnd(1),
          fanOutEnd()
        ]}
        isRunning={false}
        onSelectArtifact={noop}
      />
    );
    expect(screen.getByText("Solving for Q1")).toBeInTheDocument();
    expect(screen.queryByText("Solving for Q2")).not.toBeInTheDocument();
  });

  it("navigating to the next card shows that card's steps", () => {
    render(
      <AnalyticsReasoningTrace
        events={[
          fanOutStart(2),
          subSpecStart(0, "Q1"),
          stepStart("Solving for Q1", 0),
          stepEnd(0),
          subSpecEnd(0),
          subSpecStart(1, "Q2"),
          stepStart("Solving for Q2", 1),
          stepEnd(1),
          subSpecEnd(1),
          fanOutEnd()
        ]}
        isRunning={false}
        onSelectArtifact={noop}
      />
    );
    fireEvent.click(screen.getByLabelText("Next query"));
    expect(screen.queryByText("Solving for Q1")).not.toBeInTheDocument();
    expect(screen.getByText("Solving for Q2")).toBeInTheDocument();
  });

  it("navigating back restores the first card", () => {
    render(
      <AnalyticsReasoningTrace
        events={[
          fanOutStart(2),
          subSpecStart(0, "Q1"),
          stepStart("Solving for Q1", 0),
          stepEnd(0),
          subSpecEnd(0),
          subSpecStart(1, "Q2"),
          stepStart("Solving for Q2", 1),
          stepEnd(1),
          subSpecEnd(1),
          fanOutEnd()
        ]}
        isRunning={false}
        onSelectArtifact={noop}
      />
    );
    fireEvent.click(screen.getByLabelText("Next query"));
    fireEvent.click(screen.getByLabelText("Previous query"));
    expect(screen.getByText("Solving for Q1")).toBeInTheDocument();
    expect(screen.queryByText("Solving for Q2")).not.toBeInTheDocument();
  });

  // ── streaming mid-card ──────────────────────────────────────────────────────

  it("shows step inside an active card before sub_spec_end arrives", () => {
    render(
      <AnalyticsReasoningTrace
        events={[fanOutStart(2), subSpecStart(0, "Q1"), stepStart("Solving", 0), stepEnd(0)]}
        isRunning={true}
        onSelectArtifact={noop}
      />
    );
    expect(screen.getByText("Solving")).toBeInTheDocument();
  });

  it("shows Running placeholder when card has no steps yet", () => {
    render(
      <AnalyticsReasoningTrace
        events={[fanOutStart(2), subSpecStart(0, "Q1")]}
        isRunning={true}
        onSelectArtifact={noop}
      />
    );
    expect(screen.getByText("Running…")).toBeInTheDocument();
  });

  it("shows parallel query count in card header", () => {
    render(
      <AnalyticsReasoningTrace
        events={[fanOutStart(3), subSpecStart(0, "Q1"), stepStart("Solving", 0), stepEnd(0)]}
        isRunning={true}
        onSelectArtifact={noop}
      />
    );
    expect(screen.getByText("3 parallel queries")).toBeInTheDocument();
  });
});

// ── streaming simulation (rerender) ───────────────────────────────────────────
//
// Mirrors the real SSE flow: events are appended one-by-one and the component
// is re-rendered with each new slice, just as useAnalyticsRun does on setState.

describe("AnalyticsReasoningTrace — incremental event stream", () => {
  it("single step: transitions from streaming to done", () => {
    const events = [stepStart("Analyzing"), stepEnd()];

    const { rerender } = render(
      <AnalyticsReasoningTrace
        events={events.slice(0, 1)}
        isRunning={true}
        onSelectArtifact={noop}
      />
    );
    expect(screen.getByText("Analyzing")).toBeInTheDocument();

    rerender(
      <AnalyticsReasoningTrace
        events={events.slice(0, 2)}
        isRunning={false}
        onSelectArtifact={noop}
      />
    );
    expect(screen.getByText("Analyzing")).toBeInTheDocument();
  });

  it("sub_spec stream: correct UI at each stage", () => {
    // Pre-build the full event list in call order so seq is assigned correctly.
    const events = [
      fanOutStart(2),
      subSpecStart(0, "Q1"),
      stepStart("Solving for Q1", 0),
      stepEnd(0),
      subSpecEnd(0),
      subSpecStart(1, "Q2"),
      stepStart("Solving for Q2", 1),
      stepEnd(1),
      subSpecEnd(1),
      fanOutEnd()
    ];

    const props = (n: number, running = true) => ({
      events: events.slice(0, n),
      isRunning: running,
      onSelectArtifact: noop
    });

    const { rerender } = render(<AnalyticsReasoningTrace {...props(1)} />);

    // fan_out_start received: group present but no card yet
    expect(screen.getByText("Waiting for results…")).toBeInTheDocument();

    // sub_spec_start for Q1: card open, no steps yet
    rerender(<AnalyticsReasoningTrace {...props(2)} />);
    expect(screen.getByText("Running…")).toBeInTheDocument();

    // step_start inside Q1: step visible while streaming
    rerender(<AnalyticsReasoningTrace {...props(3)} />);
    expect(screen.getByText("Solving for Q1")).toBeInTheDocument();

    // step_end: step still visible, now marked done
    rerender(<AnalyticsReasoningTrace {...props(4)} />);
    expect(screen.getByText("Solving for Q1")).toBeInTheDocument();

    // sub_spec_end for Q1: card sealed, Q1 step still visible
    rerender(<AnalyticsReasoningTrace {...props(5)} />);
    expect(screen.getByText("Solving for Q1")).toBeInTheDocument();

    // sub_spec_start for Q2: Q2 card is now active, Q1 sealed in index 0
    rerender(<AnalyticsReasoningTrace {...props(6)} />);
    // Still showing Q1 (activeIndex is 0, unchanged)
    expect(screen.getByText("Solving for Q1")).toBeInTheDocument();

    // step_start inside Q2: navigate to see it
    rerender(<AnalyticsReasoningTrace {...props(7)} />);
    fireEvent.click(screen.getByLabelText("Next query"));
    expect(screen.getByText("Solving for Q2")).toBeInTheDocument();
    expect(screen.queryByText("Solving for Q1")).not.toBeInTheDocument();

    // fan_out_end: both cards sealed, navigation still works
    rerender(<AnalyticsReasoningTrace {...props(10, false)} />);
    expect(screen.getByText("Solving for Q2")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("Previous query"));
    expect(screen.getByText("Solving for Q1")).toBeInTheDocument();
  });

  it("sub_spec: completed card steps remain visible when next card opens", () => {
    const events = [
      fanOutStart(2),
      subSpecStart(0, "Q1"),
      stepStart("Solving for Q1", 0),
      stepEnd(0),
      subSpecEnd(0),
      subSpecStart(1, "Q2")
    ];

    const { rerender } = render(
      <AnalyticsReasoningTrace
        events={events.slice(0, 5)}
        isRunning={true}
        onSelectArtifact={noop}
      />
    );
    expect(screen.getByText("Solving for Q1")).toBeInTheDocument();

    // Q2 card opens: activeIndex is still 0, so Q1 stays visible
    rerender(
      <AnalyticsReasoningTrace
        events={events.slice(0, 6)}
        isRunning={true}
        onSelectArtifact={noop}
      />
    );
    expect(screen.getByText("Solving for Q1")).toBeInTheDocument();
    expect(screen.queryByText("Solving for Q2")).not.toBeInTheDocument();
  });
});
