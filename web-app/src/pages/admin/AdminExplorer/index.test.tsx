// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type {
  ExplorerPage,
  ExplorerQueryParams,
  ExplorerRun,
  ExplorerThread
} from "@/services/api/adminExplorer";
import AdminExplorer from "./index";

const threadsFn = vi.fn<(params: ExplorerQueryParams) => Promise<ExplorerPage<ExplorerThread>>>();
const runsFn = vi.fn<(params: ExplorerQueryParams) => Promise<ExplorerPage<ExplorerRun>>>();

vi.mock("@/services/api/adminExplorer", () => ({
  AdminExplorerService: {
    threads: (params: ExplorerQueryParams) => threadsFn(params),
    runs: (params: ExplorerQueryParams) => runsFn(params)
  }
}));

const THREAD: ExplorerThread = {
  id: "thread-1",
  title: "Why is the revenue dashboard showing negative numbers for EMEA?",
  input_snippet: "Can you check why the EMEA revenue chart is showing negative values?",
  source_type: "agent",
  is_processing: true,
  created_at: new Date(Date.now() - 5 * 60_000).toISOString(),
  user_email: "operator0@acme.io",
  workspace_id: "ws-1",
  workspace_name: "Acme Workspace 1",
  org_id: "org-1",
  org_name: "Acme Corp 1",
  org_slug: "acme-1"
};

const FAILED_RUN: ExplorerRun = {
  id: "run-1",
  question_snippet: "What was the week-over-week change in active users?",
  task_status: "failed",
  source_type: "analytics",
  error_message:
    "OxyError::Database: connection to warehouse timed out after 30s\n  at crates/agentic/connector/src/postgres.rs:142",
  created_at: new Date(Date.now() - 3 * 60_000).toISOString(),
  thread_id: "thread-1",
  workspace_id: "ws-1",
  workspace_name: "Acme Workspace 1",
  org_id: "org-1",
  org_name: "Acme Corp 1",
  org_slug: "acme-1",
  user_email: "operator0@acme.io"
};

const DONE_RUN: ExplorerRun = {
  ...FAILED_RUN,
  id: "run-2",
  task_status: "done",
  error_message: null,
  question_snippet: "How many active users last week?"
};

function page<T>(items: T[], total = items.length): ExplorerPage<T> {
  return { items, total, page: 1, page_size: 25 };
}

function manyThreads(count: number, idOffset = 0): ExplorerThread[] {
  return Array.from({ length: count }, (_, i) => ({ ...THREAD, id: `thread-${idOffset + i}` }));
}

const wrap = (ui: ReactNode) => {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <MemoryRouter>
      <QueryClientProvider client={qc}>{ui}</QueryClientProvider>
    </MemoryRouter>
  );
};

beforeAll(() => {
  // jsdom doesn't implement these, but Radix Select needs them.
  window.HTMLElement.prototype.scrollIntoView = vi.fn();
  window.HTMLElement.prototype.hasPointerCapture = vi.fn();
  window.HTMLElement.prototype.releasePointerCapture = vi.fn();
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("AdminExplorer", () => {
  it("renders the threads table by default", async () => {
    threadsFn.mockResolvedValue(page([THREAD]));
    render(wrap(<AdminExplorer />));

    expect(await screen.findByText(/Why is the revenue dashboard/)).toBeInTheDocument();
    expect(screen.getByText("Acme Workspace 1 · Acme Corp 1")).toBeInTheDocument();
    expect(screen.getByText("operator0@acme.io")).toBeInTheDocument();
    expect(screen.getByText("live")).toBeInTheDocument();
    expect(screen.getByText("1 result")).toBeInTheDocument();
  });

  it("switches to the runs tab and expands a failed row to show its error", async () => {
    threadsFn.mockResolvedValue(page([THREAD]));
    runsFn.mockResolvedValue(page([FAILED_RUN, DONE_RUN]));
    render(wrap(<AdminExplorer />));

    await screen.findByText(/Why is the revenue dashboard/);
    await userEvent.click(screen.getByRole("button", { name: "Runs" }));

    expect(await screen.findByText(/week-over-week change/)).toBeInTheDocument();
    expect(screen.getByText("failed")).toBeInTheDocument();
    expect(screen.getByText("done")).toBeInTheDocument();

    // error detail is collapsed until the row is clicked
    expect(screen.queryByText(/connection to warehouse timed out/)).not.toBeInTheDocument();

    await userEvent.click(screen.getByText(/week-over-week change in active users\?/));

    expect(await screen.findByText(/connection to warehouse timed out/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Open thread/ })).toBeInTheDocument();
  });

  it("debounces the search box into the query params", async () => {
    threadsFn.mockResolvedValue(page([THREAD]));
    render(wrap(<AdminExplorer />));
    await screen.findByText(/Why is the revenue dashboard/);
    threadsFn.mockClear();

    await userEvent.type(screen.getByLabelText("Search"), "EMEA");

    await waitFor(() => {
      expect(threadsFn).toHaveBeenCalledWith(expect.objectContaining({ search: "EMEA" }));
    });
  });

  it("applies the status filter as a query param and resets to page 1", async () => {
    threadsFn.mockResolvedValue(page([THREAD]));
    render(wrap(<AdminExplorer />));
    await screen.findByText(/Why is the revenue dashboard/);
    threadsFn.mockClear();

    await userEvent.click(screen.getByLabelText("Status filter"));
    await userEvent.click(await screen.findByRole("option", { name: "Live" }));

    await waitFor(() => {
      expect(threadsFn).toHaveBeenCalledWith(expect.objectContaining({ status: "live", page: 1 }));
    });
  });

  it("renders pagination and requests the next page", async () => {
    threadsFn.mockResolvedValue(page(manyThreads(25), 60));
    render(wrap(<AdminExplorer />));
    await screen.findByText("60 results");

    threadsFn.mockResolvedValue(page(manyThreads(25, 25), 60));
    await userEvent.click(screen.getByRole("link", { name: "2" }));

    await waitFor(() => {
      expect(threadsFn).toHaveBeenCalledWith(expect.objectContaining({ page: 2 }));
    });
  });

  it("clamps back into range when the current page falls out of bounds", async () => {
    // Page 1 has rows (total 60); any deeper page comes back empty with
    // total 0, simulating the match set shrinking under a deep page.
    threadsFn.mockImplementation((params: ExplorerQueryParams) =>
      Promise.resolve(
        (params.page ?? 1) >= 2 ? page<ExplorerThread>([], 0) : page(manyThreads(25), 60)
      )
    );
    render(wrap(<AdminExplorer />));
    await screen.findByText("60 results");

    await userEvent.click(screen.getByRole("link", { name: "2" }));

    // The out-of-range page-2 fetch returns total 0, so the clamp bounces
    // back to a valid page rather than stranding the user on an empty one —
    // page 1 becomes active again and the real count is restored. (Page 1 is
    // served from cache on the bounce-back, so the mock isn't re-invoked.)
    await waitFor(() => {
      expect(threadsFn).toHaveBeenCalledWith(expect.objectContaining({ page: 2 }));
    });
    await waitFor(() => {
      expect(screen.getByRole("link", { name: "1" })).toHaveAttribute("aria-current", "page");
    });
    expect(screen.getByText("60 results")).toBeInTheDocument();
  });
});
