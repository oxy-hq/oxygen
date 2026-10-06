// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useWorkspaceHealthHistory } from "@/hooks/api/workspaceHealth/useWorkspaceHealthHistory";
import type { WorkspaceHealthHistory } from "@/services/api/workspaceHealth";
import { HealthHistory } from "./HealthHistory";

vi.mock("@/hooks/api/workspaceHealth/useWorkspaceHealthHistory", () => ({
  HEALTH_HISTORY_DAYS: 30,
  useWorkspaceHealthHistory: vi.fn()
}));

type HistoryQuery = ReturnType<typeof useWorkspaceHealthHistory>;

const HOUR = 3_600_000;
// The clock is pinned: the fixtures and the component each read it, and a
// millisecond between two reads turns a 3h stretch into "2h 59m".
const NOW = new Date("2026-10-06T12:00:00Z").getTime();
const ago = (ms: number) => new Date(NOW - ms).toISOString();

const answerWith = (answer: Partial<HistoryQuery>) =>
  vi.mocked(useWorkspaceHealthHistory).mockReturnValue({
    data: undefined,
    isPending: false,
    isError: false,
    error: null,
    refetch: vi.fn(),
    ...answer
  } as unknown as HistoryQuery);

const LABELS: Record<string, string> = { pipeline: "Pipeline", queue: "Queue" };
const mount = () => render(<HealthHistory workspaceId='w1' labelOf={(d) => LABELS[d] ?? d} />);

const history = (over: Partial<WorkspaceHealthHistory>): WorkspaceHealthHistory => ({
  window_days: 30,
  transitions: [],
  opening: null,
  truncated: false,
  ...over
});

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(NOW);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.useRealTimers();
});

describe("HealthHistory", () => {
  it("lists each stretch with how long it lasted and what was failing when it began", () => {
    answerWith({
      data: history({
        opening: { at: ago(60 * 24 * HOUR), from_status: null, to_status: "healthy", failures: [] },
        transitions: [
          { at: ago(2 * HOUR), from_status: "unhealthy", to_status: "healthy", failures: [] },
          {
            at: ago(5 * HOUR),
            from_status: "healthy",
            to_status: "unhealthy",
            failures: [
              { dimension: "pipeline", status: "unhealthy" },
              // A dimension retired since the row was written.
              { dimension: "correctness", status: "unhealthy" }
            ]
          }
        ]
      })
    });
    mount();

    expect(screen.getByTestId("admin-workspace-health-history-summary")).toHaveTextContent(
      "In the last 30 days: unhealthy once, 3h in total."
    );
    const rows = screen.getAllByTestId("admin-workspace-health-history-row");
    expect(rows).toHaveLength(3);
    expect(rows[0]).toHaveTextContent("healthy");
    expect(rows[0]).toHaveTextContent("2h so far");
    expect(rows[1]).toHaveTextContent("unhealthy");
    expect(rows[1]).toHaveTextContent("3h");
    expect(rows[1]).not.toHaveTextContent("so far");
    expect(rows[1]).toHaveTextContent("Pipeline, correctness");
  });

  it("marks a stretch that began before the window", () => {
    answerWith({
      data: history({
        opening: {
          at: ago(40 * 24 * HOUR),
          from_status: "healthy",
          to_status: "degraded",
          failures: [{ dimension: "queue", status: "degraded" }]
        }
      })
    });
    mount();

    const row = screen.getByTestId("admin-workspace-health-history-row");
    expect(row).toHaveTextContent(/before /);
    expect(row).toHaveTextContent("30d so far");
    expect(row).toHaveTextContent("Queue");
    expect(
      screen.queryByTestId("admin-workspace-health-history-truncated")
    ).not.toBeInTheDocument();
  });

  it("says when the list was cut, and totals nothing", () => {
    answerWith({
      data: history({
        transitions: [
          { at: ago(1 * HOUR), from_status: "unhealthy", to_status: "healthy", failures: [] },
          { at: ago(2 * HOUR), from_status: "healthy", to_status: "unhealthy", failures: [] }
        ],
        truncated: true
      })
    });
    mount();

    expect(screen.getByTestId("admin-workspace-health-history-truncated")).toHaveTextContent(
      "Only the 2 most recent changes are listed."
    );
    expect(screen.getByTestId("admin-workspace-health-history-summary")).not.toHaveTextContent(
      "in total"
    );
    expect(screen.getAllByTestId("admin-workspace-health-history-row")).toHaveLength(2);
  });

  // A history that could not be read is not "no changes".
  it("tells a failed read from an empty history", () => {
    answerWith({ isError: true, error: new Error("history query failed") });
    const { unmount } = mount();
    expect(screen.getByTestId("admin-async-error")).toHaveTextContent("history query failed");
    expect(screen.queryByTestId("admin-workspace-health-history-summary")).not.toBeInTheDocument();
    unmount();

    answerWith({ data: history({}) });
    mount();
    expect(screen.getByTestId("admin-workspace-health-history-summary")).toHaveTextContent(
      "No status change has been recorded for this workspace yet."
    );
    expect(screen.queryByTestId("admin-workspace-health-history-row")).not.toBeInTheDocument();
  });
});
