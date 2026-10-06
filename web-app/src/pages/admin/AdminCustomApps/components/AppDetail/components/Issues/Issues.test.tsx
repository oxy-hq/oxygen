// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useAppIssues } from "@/hooks/api/customApps/useAppIssues";
import type { AppIssue, AppIssueList } from "@/services/api/appIssues";
import type { CustomApp } from "@/types/apps";
import { Issues, IssuesBadge } from "./index";

vi.mock("@/hooks/api/customApps/useAppIssues", () => ({
  ISSUE_WINDOW_DAYS: 7,
  useAppIssues: vi.fn()
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

type IssuesQuery = ReturnType<typeof useAppIssues>;

const APP = { id: "app-id", org_slug: "acme", slug: "bookkeeping" } as CustomApp;

function issue(overrides: Partial<AppIssue> = {}): AppIssue {
  return {
    function_name: "upload-report",
    fingerprint: "5c1e0b8a9d2f4e71",
    occurrences: 1234,
    first_seen: "2026-10-01T08:00:00+00:00",
    last_seen: "2026-10-03T14:02:11+00:00",
    builds: 2,
    on_live_build: true,
    last: {
      invocation_id: "0b7a1c2e-5d3f-4a6b-8c9d-0e1f2a3b4c5d",
      status: "error",
      result_status: null,
      error: "function threw: Error: warehouse insert failed",
      build_id: "b2",
      created_at: "2026-10-03T14:02:11+00:00"
    },
    ...overrides
  };
}

const answerWith = (answer: Partial<IssuesQuery>) =>
  vi.mocked(useAppIssues).mockReturnValue({
    data: undefined,
    isPending: false,
    isError: false,
    error: null,
    refetch: vi.fn(),
    ...answer
  } as unknown as IssuesQuery);

const listOf = (issues: AppIssue[], truncated = false): AppIssueList => ({
  window_days: 7,
  issues,
  truncated
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("Issues", () => {
  it("shows one row per distinct failure and says whether the live build has had it", () => {
    answerWith({
      data: listOf([issue(), issue({ function_name: "sync-orders", on_live_build: false })])
    });
    render(<Issues app={APP} onOpenFunction={vi.fn()} />);

    const live = screen.getByTestId("admin-app-issues-row-upload-report-5c1e0b8a9d2f4e71");
    const replaced = screen.getByTestId("admin-app-issues-row-sync-orders-5c1e0b8a9d2f4e71");
    expect(live).toHaveTextContent("1,234");
    expect(live).toHaveTextContent("On the live build");
    expect(live).not.toHaveTextContent("Not on the live build");
    expect(replaced).toHaveTextContent("Not on the live build");
    expect(screen.queryByTestId("admin-app-issues-truncated")).not.toBeInTheDocument();
  });

  it("tells an app with no failures from a failed read", () => {
    answerWith({ data: listOf([]) });
    const { unmount } = render(<Issues app={APP} onOpenFunction={vi.fn()} />);
    expect(screen.getByTestId("admin-app-issues-empty")).toBeInTheDocument();
    unmount();

    // A server that did not answer is not "nothing is failing".
    answerWith({ isError: true, error: new Error("issue query failed") });
    render(<Issues app={APP} onOpenFunction={vi.fn()} />);
    expect(screen.queryByTestId("admin-app-issues-empty")).not.toBeInTheDocument();
    expect(screen.getByTestId("admin-async-error")).toHaveTextContent("issue query failed");
  });

  it("says so when the list is cut", () => {
    answerWith({ data: listOf([issue()], true) });
    render(<Issues app={APP} onOpenFunction={vi.fn()} />);
    expect(screen.getByTestId("admin-app-issues-truncated")).toHaveTextContent(
      "More distinct failures happened in the last 7 days"
    );
  });

  // Function plus fingerprint is what the pager quotes, so both have to be
  // readable before anything is opened — two failures of one function would
  // otherwise be the same row twice. Grouping is spacing only: the text is
  // still the one unbroken value a search or a paste would look for.
  it("identifies a closed row by function and fingerprint, and keeps the failure text for the open one", () => {
    answerWith({ data: listOf([issue()]) });
    render(<Issues app={APP} onOpenFunction={vi.fn()} />);
    const row = screen.getByTestId("admin-app-issues-row-upload-report-5c1e0b8a9d2f4e71");
    const toggle = screen.getByTestId("admin-app-issues-row-toggle");

    expect(row).toHaveTextContent("upload-report5c1e0b8a9d2f4e71");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByTestId("admin-app-issues-row-error")).not.toBeInTheDocument();

    fireEvent.click(toggle);

    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByTestId("admin-app-issues-row-error")).toHaveTextContent(
      "function threw: Error: warehouse insert failed"
    );
    expect(row).toHaveTextContent("0b7a1c2e-5d3f-4a6b-8c9d-0e1f2a3b4c5d");
    expect(row).toHaveTextContent("2, last on b2");
  });

  it("opens the failing function's invocations", () => {
    const onOpenFunction = vi.fn();
    answerWith({ data: listOf([issue()]) });
    render(<Issues app={APP} onOpenFunction={onOpenFunction} />);

    fireEvent.click(screen.getByTestId("admin-app-issues-row-toggle"));
    fireEvent.click(screen.getByTestId("admin-app-issues-row-open-function"));
    expect(onOpenFunction).toHaveBeenCalledWith("upload-report");
  });

  it("copies the issue as a brief an agent can act on", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    answerWith({ data: listOf([issue()]) });
    render(<Issues app={APP} onOpenFunction={vi.fn()} />);

    fireEvent.click(screen.getByTestId("admin-app-issues-row-toggle"));
    fireEvent.click(screen.getByTestId("admin-app-issues-row-copy"));

    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    const brief = writeText.mock.calls[0][0] as string;
    expect(brief).toContain("Custom app issue: acme/bookkeeping");
    expect(brief).toContain("Function:      upload-report");
    expect(brief).toContain("oxyc api");
  });
});

describe("IssuesBadge", () => {
  it("counts the issues the live build has had, and is absent at zero", () => {
    answerWith({
      data: listOf([
        issue(),
        issue({ function_name: "sync-orders" }),
        issue({ function_name: "nightly", on_live_build: false })
      ])
    });
    const { unmount } = render(<IssuesBadge appId='app-id' />);
    expect(screen.getByTestId("admin-app-issues-live-badge")).toHaveTextContent("2 on live build");
    unmount();

    // History only: nothing the live build has had. A lit badge here would be
    // lit on every app that ever had a bug.
    answerWith({ data: listOf([issue({ on_live_build: false })]) });
    render(<IssuesBadge appId='app-id' />);
    expect(screen.queryByTestId("admin-app-issues-live-badge")).not.toBeInTheDocument();
  });

  it("shows nothing while the answer is not in", () => {
    answerWith({ isPending: true });
    render(<IssuesBadge appId='app-id' />);
    expect(screen.queryByTestId("admin-app-issues-live-badge")).not.toBeInTheDocument();
  });
});
