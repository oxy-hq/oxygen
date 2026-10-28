// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { WorkspaceSummary } from "@/services/api/workspaces";
import { WorkspaceStats } from "./WorkspaceStats";

// `null` means the answering instance holds no working copy and did not count;
// `0` means it counted and found none. Both used to render as no stats row, so
// a workspace with seventeen agents looked exactly like an empty one.

const workspace = (
  counts: Pick<WorkspaceSummary, "agent_count" | "workflow_count">
): WorkspaceSummary =>
  ({
    id: "w1",
    org_id: null,
    name: "Demo",
    path: "/w",
    created_at: "2026-08-25T00:00:00Z",
    last_opened_at: null,
    created_by_name: null,
    status: "ready",
    error: null,
    ...counts
  }) as WorkspaceSummary;

afterEach(cleanup);

describe("WorkspaceStats", () => {
  it("says nothing was counted when the instance could not look", () => {
    render(<WorkspaceStats workspace={workspace({ agent_count: null, workflow_count: null })} />);
    expect(screen.getByTestId("workspace-stats-unknown")).toBeInTheDocument();
  });

  it("renders no row for a workspace that was counted and is empty", () => {
    const { container } = render(
      <WorkspaceStats workspace={workspace({ agent_count: 0, workflow_count: 0 })} />
    );
    expect(screen.queryByTestId("workspace-stats-unknown")).not.toBeInTheDocument();
    expect(container).toBeEmptyDOMElement();
  });

  it("renders the counts when there are some", () => {
    render(<WorkspaceStats workspace={workspace({ agent_count: 17, workflow_count: 23 })} />);
    expect(screen.queryByTestId("workspace-stats-unknown")).not.toBeInTheDocument();
    expect(screen.getByText("17")).toBeInTheDocument();
    expect(screen.getByText("23")).toBeInTheDocument();
  });

  it("separates the two states that both mean 'no numbers to show'", () => {
    const { container: unknown } = render(
      <WorkspaceStats workspace={workspace({ agent_count: null, workflow_count: null })} />
    );
    const unknownHtml = unknown.innerHTML;
    cleanup();
    const { container: empty } = render(
      <WorkspaceStats workspace={workspace({ agent_count: 0, workflow_count: 0 })} />
    );
    expect(unknownHtml).not.toEqual(empty.innerHTML);
  });
});
