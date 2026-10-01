// @vitest-environment jsdom

/**
 * Settings → Previews, driven down to the HTTP calls: the assertions are on
 * what reaches `apiClient` (method, path, body, params), because the contract
 * with the backend is the endpoint, not the service function's name.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createElement } from "react";
import { MemoryRouter, Route, Routes, useLocation, useParams } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, type Mock, vi } from "vitest";
import type { Workspace, WorkspacePreview } from "@/types/workspace";

const mocks = vi.hoisted(() => ({
  staff: true,
  get: vi.fn(),
  post: vi.fn(),
  delete: vi.fn(),
  toastError: vi.fn()
}));

vi.mock("@/hooks/useCanUsePreviews", () => ({ default: () => mocks.staff }));
vi.mock("@/services/api/axios", () => ({
  apiClient: { get: mocks.get, post: mocks.post, delete: mocks.delete }
}));
vi.mock("sonner", () => ({ toast: { error: mocks.toastError, success: vi.fn() } }));

import { AxiosError, AxiosHeaders, type InternalAxiosRequestConfig } from "axios";

/** What the backend (#3378) answers when it refuses a create or refresh. */
function apiError(status: number, data: unknown) {
  const config = { headers: new AxiosHeaders() } as InternalAxiosRequestConfig;
  return new AxiosError(
    `Request failed with status code ${status}`,
    "ERR_BAD_REQUEST",
    config,
    null,
    {
      status,
      statusText: "",
      headers: new AxiosHeaders(),
      config,
      data
    }
  );
}

const CANNOT_COMPILE = {
  code: "cannot_compile",
  message: "feat/x has uncommitted edits in its worktree. Commit or discard them, then refresh."
};

import { PreviewPinProvider } from "@/contexts/PreviewPinContext";
import { PREVIEW_POLL_MS } from "@/hooks/api/workspaces/usePreviews";
import Previews from "./index";

const WS = { id: "ws-1", name: "Analytics", default_branch: "main" } as Workspace;
// The literal contract path, not `previewsPath`: a test that echoes the
// constant cannot notice the route moving.
const LIST = "/ws-1/previews";

const row = (over: Partial<WorkspacePreview>): WorkspacePreview => ({
  branch: "feat/x",
  status: "ready",
  revision_id: "rev-1",
  sha: "abc1234def",
  error: null,
  created_by: { id: "u1", name: "Ada" },
  updated_at: "2026-09-28T10:00:00Z",
  compiled_at: "2026-09-28T10:00:00Z",
  checks: null,
  ...over
});

/**
 * Serve these rows from GET on the previews-list endpoint, one response per
 * call, repeating the last. Any other GET the tab now also fires on mount
 * (the Sandbox sources panel's `/sources`) gets a stable empty list instead
 * of sharing this counter — otherwise its one extra call at mount shifts
 * every later index by one and the list's own poll count comes out wrong.
 */
function serve(...responses: WorkspacePreview[][]) {
  let call = 0;
  mocks.get.mockImplementation((url: string) => {
    if (url !== LIST) return Promise.resolve({ data: [] });
    const items = responses[Math.min(call, responses.length - 1)];
    call += 1;
    return Promise.resolve({ data: { items } });
  });
}

const listCalls = () => mocks.get.mock.calls.filter(([url]) => url === LIST).length;

let client: QueryClient;
let onOpened: Mock<() => void>;

function Url() {
  const location = useLocation();
  return createElement("span", { "data-testid": "url" }, `${location.pathname}${location.search}`);
}

function Layout() {
  const { wsId } = useParams<{ wsId: string }>();
  return (
    <PreviewPinProvider workspaceId={wsId ?? ""}>
      <Previews workspace={WS} onOpened={onOpened} />
      <Url />
    </PreviewPinProvider>
  );
}

function renderTab() {
  return render(
    createElement(
      QueryClientProvider,
      { client },
      createElement(
        MemoryRouter,
        { initialEntries: ["/acme/workspaces/ws-1/apps"] },
        createElement(
          Routes,
          null,
          createElement(Route, { path: "/acme/workspaces/:wsId/*", element: createElement(Layout) })
        )
      )
    )
  );
}

const rowEl = (branch: string) => screen.getByTestId(`preview-row-${branch}`);

describe("Settings → Previews", () => {
  beforeEach(() => {
    mocks.staff = true;
    for (const m of [mocks.get, mocks.post, mocks.delete, mocks.toastError]) m.mockReset();
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    onOpened = vi.fn<() => void>();
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("shows each preview's status, author and last compile", async () => {
    // A zone-less timestamp is UTC. Read as local time it would be off by the
    // viewer's offset — "5 hours ago" — instead of five minutes.
    const fiveMinutesAgo = new Date(Date.now() - 5 * 60_000).toISOString().replace("Z", "");
    serve([
      row({ branch: "feat/a", status: "compiling", compiled_at: null, created_by: null }),
      row({ branch: "feat/b", status: "ready", compiled_at: fiveMinutesAgo }),
      row({ branch: "feat/c", status: "failed", error: "orders.view.yml: unknown measure" }),
      row({ branch: "feat/d", status: "ready", compiled_at: "not a date" })
    ]);
    renderTab();

    await screen.findByTestId("preview-row-feat/a");
    expect(mocks.get).toHaveBeenCalledWith(LIST);

    const compiling = rowEl("feat/a");
    expect(compiling).toHaveAttribute("data-status", "compiling");
    expect(within(compiling).getByTestId("preview-status")).toHaveTextContent("Compiling");
    expect(compiling).toHaveTextContent("Never");
    expect(within(compiling).queryByText("Open")).toBeNull();

    const ready = rowEl("feat/b");
    expect(within(ready).getByTestId("preview-status")).toHaveTextContent("Ready");
    expect(ready).toHaveTextContent("Ada");
    expect(ready).toHaveTextContent("5 minutes ago");
    expect(within(ready).getByTestId("preview-row-feat/b-open")).toBeInTheDocument();

    expect(rowEl("feat/d")).toHaveTextContent("—");

    const failed = rowEl("feat/c");
    expect(within(failed).getByTestId("preview-status")).toHaveTextContent("Failed");
    expect(within(failed).queryByText("Open")).toBeNull();
    expect(screen.queryByText("orders.view.yml: unknown measure")).toBeNull();
    fireEvent.click(within(failed).getByTestId("preview-row-feat/c-toggle-error"));
    expect(screen.getByTestId("preview-row-feat/c-error")).toHaveTextContent(
      "orders.view.yml: unknown measure"
    );
  });

  it("Open pins the current page to the row's compiled revision and closes the dialog", async () => {
    serve([row({ branch: "feat/b", revision_id: "rev-b" })]);
    renderTab();

    fireEvent.click(await screen.findByTestId("preview-row-feat/b-open"));

    expect(onOpened).toHaveBeenCalled();
    // The revision, not the branch: a shared link keeps opening this compile.
    expect(screen.getByTestId("url")).toHaveTextContent("/acme/workspaces/ws-1/apps?preview=rev-b");
  });

  // `stale` is two situations with one fix: the head moved past a compiled
  // revision, or there is no revision and nothing queued (a compile that died
  // before starting, or a revision retention removed).
  it.each([
    ["the branch moved past its revision", { revision_id: "rev-old", sha: "9f8e7d6c5b" }],
    ["there is no revision and nothing queued", { revision_id: null, sha: null }]
  ] as const)("prompts a refresh when a preview is stale because %s", async (_, fields) => {
    const compiling = row({ branch: "feat/s", status: "compiling", sha: null });
    serve([row({ branch: "feat/s", status: "stale", ...fields })]);
    mocks.post.mockImplementation(() => {
      serve([compiling]);
      return Promise.resolve({ data: { item: compiling } });
    });
    renderTab();

    const stale = await screen.findByTestId("preview-row-feat/s");
    expect(within(stale).getByTestId("preview-status")).toHaveTextContent("Stale");
    if (fields.sha) expect(stale).toHaveTextContent("@ 9f8e7d6");
    // A stale revision is not offered for opening: the branch has moved past it.
    expect(within(stale).queryByText("Open")).toBeNull();

    fireEvent.click(within(stale).getByTestId("preview-row-feat/s-stale-refresh"));

    await waitFor(() =>
      expect(mocks.post).toHaveBeenCalledWith(`${LIST}/refresh`, undefined, {
        params: { branch: "feat/s" }
      })
    );
    await waitFor(() => expect(rowEl("feat/s")).toHaveAttribute("data-status", "compiling"));
  });

  it("Refresh recompiles the branch", async () => {
    const compiling = row({ branch: "feat/c", status: "compiling" });
    serve([row({ branch: "feat/c", status: "failed", error: "boom" })]);
    mocks.post.mockImplementation(() => {
      serve([compiling]);
      return Promise.resolve({ data: { item: compiling } });
    });
    renderTab();

    fireEvent.click(await screen.findByTestId("preview-row-feat/c-refresh"));

    await waitFor(() =>
      expect(mocks.post).toHaveBeenCalledWith(`${LIST}/refresh`, undefined, {
        params: { branch: "feat/c" }
      })
    );
    await waitFor(() => expect(rowEl("feat/c")).toHaveAttribute("data-status", "compiling"));
  });

  it("Delete asks first, then removes the preview", async () => {
    serve([row({ branch: "feat/b" })]);
    mocks.delete.mockResolvedValue({ status: 204 });
    renderTab();

    fireEvent.click(await screen.findByTestId("preview-row-feat/b-delete"));
    expect(mocks.delete).not.toHaveBeenCalled();
    fireEvent.click(await screen.findByTestId("delete-preview-confirm"));

    await waitFor(() =>
      expect(mocks.delete).toHaveBeenCalledWith(LIST, { params: { branch: "feat/b" } })
    );
  });

  it("New preview creates one from a branch name", async () => {
    const created = row({ branch: "feat/new", status: "compiling" });
    serve([]);
    mocks.post.mockImplementation(() => {
      serve([created]);
      return Promise.resolve({ data: { item: created } });
    });
    renderTab();
    await screen.findByText("No previews yet");

    fireEvent.click(screen.getByTestId("new-preview-submit"));
    expect(await screen.findByText("Enter a branch name.")).toBeInTheDocument();
    expect(mocks.post).not.toHaveBeenCalled();

    fireEvent.change(screen.getByTestId("new-preview-branch"), {
      target: { value: "  feat/new " }
    });
    fireEvent.click(screen.getByTestId("new-preview-submit"));

    await waitFor(() => expect(mocks.post).toHaveBeenCalledWith(LIST, { branch: "feat/new" }));
    expect(await screen.findByTestId("preview-row-feat/new")).toHaveAttribute(
      "data-status",
      "compiling"
    );
  });

  it("polls while a preview compiles, and stops once none is", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    serve([row({ branch: "feat/a", status: "compiling" })], [row({ branch: "feat/a" })]);
    renderTab();
    await screen.findByTestId("preview-row-feat/a");
    expect(listCalls()).toBe(1);

    await act(() => vi.advanceTimersByTimeAsync(PREVIEW_POLL_MS));
    await waitFor(() => expect(rowEl("feat/a")).toHaveAttribute("data-status", "ready"));
    expect(listCalls()).toBe(2);

    // Nothing compiling any more: a long quiet stretch reads nothing.
    await act(() => vi.advanceTimersByTimeAsync(PREVIEW_POLL_MS * 10));
    expect(listCalls()).toBe(2);
  });

  // S12 fix round 1: a row's compile can settle while its checks analysis is
  // still running — the Checks panel only ever refreshes via this list poll,
  // so if the list stops polling the moment `status` leaves "compiling", the
  // badge is stuck on "checking…" forever.
  it("keeps polling while a settled row's checks are still pending, and stops once they settle", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    serve(
      [
        row({
          branch: "feat/a",
          status: "ready",
          checks: { status: "pending", needs_reset: 0, warnings: 0, transforms: 0 }
        })
      ],
      [
        row({
          branch: "feat/a",
          status: "ready",
          checks: { status: "done", needs_reset: 0, warnings: 0, transforms: 0 }
        })
      ]
    );
    renderTab();
    await screen.findByTestId("preview-row-feat/a");
    expect(listCalls()).toBe(1);

    await act(() => vi.advanceTimersByTimeAsync(PREVIEW_POLL_MS));
    await waitFor(() => expect(listCalls()).toBe(2));

    // Checks settled now: a long quiet stretch reads nothing.
    await act(() => vi.advanceTimersByTimeAsync(PREVIEW_POLL_MS * 10));
    expect(listCalls()).toBe(2);
  });

  describe("what #3378 answers a create or refresh with", () => {
    it("says why a refresh cannot compile on the row, in the server's words — not as a toast", async () => {
      serve([row({ branch: "feat/x", status: "stale" })]);
      mocks.post.mockRejectedValue(apiError(409, CANNOT_COMPILE));
      renderTab();

      fireEvent.click(await screen.findByTestId("preview-row-feat/x-refresh"));

      expect(await screen.findByTestId("preview-row-feat/x-notice")).toHaveTextContent(
        CANNOT_COMPILE.message
      );
      expect(mocks.toastError).not.toHaveBeenCalled();
      // Still the row it was: nothing started compiling.
      expect(rowEl("feat/x")).toHaveAttribute("data-status", "stale");
    });

    it("says why a new preview cannot compile on the branch field, and keeps what was typed", async () => {
      serve([]);
      mocks.post.mockRejectedValue(apiError(409, CANNOT_COMPILE));
      renderTab();
      await screen.findByText("No previews yet");

      fireEvent.change(screen.getByTestId("new-preview-branch"), { target: { value: "feat/x" } });
      fireEvent.click(screen.getByTestId("new-preview-submit"));

      expect(await screen.findByTestId("new-preview-error")).toHaveTextContent(
        CANNOT_COMPILE.message
      );
      expect(screen.getByTestId("new-preview-branch")).toHaveValue("feat/x");
      expect(mocks.toastError).not.toHaveBeenCalled();
    });

    it("offers Create when a refreshed branch was never previewed", async () => {
      const created = row({ branch: "feat/gone", status: "compiling", revision_id: null });
      serve([row({ branch: "feat/gone", status: "failed", error: "boom" })]);
      mocks.post.mockImplementation((path: string) => {
        if (path === `${LIST}/refresh`) {
          return Promise.reject(apiError(404, { code: "preview_not_found" }));
        }
        serve([created]);
        return Promise.resolve({ data: { item: created } });
      });
      renderTab();

      fireEvent.click(await screen.findByTestId("preview-row-feat/gone-refresh"));

      const notice = await screen.findByTestId("preview-row-feat/gone-notice");
      expect(notice).toHaveTextContent("This branch hasn't been previewed yet — create it.");
      expect(mocks.toastError).not.toHaveBeenCalled();

      fireEvent.click(within(notice).getByTestId("preview-row-feat/gone-create"));

      await waitFor(() => expect(mocks.post).toHaveBeenCalledWith(LIST, { branch: "feat/gone" }));
      await waitFor(() => expect(rowEl("feat/gone")).toHaveAttribute("data-status", "compiling"));
      expect(screen.queryByTestId("preview-row-feat/gone-notice")).toBeNull();
    });

    it("shows a refresh of an unchanged head as ready at once — never compiling, never polling", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      const ready = row({ branch: "feat/x", status: "ready", revision_id: "rev-1" });
      serve([ready]);
      mocks.post.mockResolvedValue({ data: { item: ready } });
      renderTab();
      await screen.findByTestId("preview-row-feat/x");

      fireEvent.click(screen.getByTestId("preview-row-feat/x-refresh"));
      await waitFor(() => expect(mocks.post).toHaveBeenCalled());
      // The first load, then the one re-read every successful refresh makes.
      await waitFor(() => expect(listCalls()).toBe(2));

      expect(rowEl("feat/x")).toHaveAttribute("data-status", "ready");
      expect(screen.queryByText("Compiling")).toBeNull();
      await act(() => vi.advanceTimersByTimeAsync(PREVIEW_POLL_MS * 5));
      expect(listCalls()).toBe(2);
      expect(screen.queryByText("Compiling")).toBeNull();
    });
  });

  // S12: `serve()` above ignores the URL (fine, since none of those tests hit
  // anything but the list), so these two route by path explicitly to exercise
  // the Checks and Runs panels this row now embeds.
  describe("Checks and Runs (S12)", () => {
    it("shows the row's embedded checks verdict, and expands it to per-pipeline detail", async () => {
      mocks.get.mockImplementation((url: string) => {
        if (url === LIST) {
          return Promise.resolve({
            data: {
              items: [
                row({
                  branch: "feat/x",
                  checks: { status: "done", needs_reset: 1, warnings: 0, transforms: 0 }
                })
              ]
            }
          });
        }
        if (url === `${LIST}/checks`) {
          return Promise.resolve({
            data: {
              branch: "feat/x",
              revision_id: "rev-1",
              status: "done",
              error: null,
              pipelines: [
                {
                  name: "toast",
                  file_path: "airway/toast.airway.yml",
                  change: "modified",
                  verdict: "needs_reset",
                  findings: [
                    {
                      kind: "WriteDispositionChanged",
                      verdict: "needs_reset",
                      detail: "orders: append → merge",
                      prod_action: "Reset schema, then backfill"
                    }
                  ]
                }
              ],
              transforms: []
            }
          });
        }
        return Promise.resolve({ data: [] });
      });
      renderTab();

      const toggle = await screen.findByTestId("preview-row-feat/x-checks-toggle");
      expect(toggle).toHaveTextContent("1 need reset");

      fireEvent.click(toggle);
      expect(
        await screen.findByText(/Prod action: Reset schema, then backfill/)
      ).toBeInTheDocument();
    });

    it("opens the Runs panel from the row, lists prior runs, and starts a dry-run", async () => {
      mocks.get.mockImplementation((url: string) => {
        if (url === LIST) return Promise.resolve({ data: { items: [row({ branch: "feat/x" })] } });
        if (url === `${LIST}/runs`) {
          return Promise.resolve({
            data: [
              {
                run_id: "run-1",
                branch: "feat/x",
                kind: "procedure",
                target_ref: "workflows/existing.procedure.yml",
                revision_id: "rev-1",
                state: "finished",
                outcome: "succeeded",
                held_count: 2,
                requested_by: null,
                created_at: "2026-09-28T10:00:00Z",
                started_at: null,
                finished_at: null,
                parent_run_id: null
              }
            ]
          });
        }
        return Promise.resolve({ data: [] });
      });
      mocks.post.mockResolvedValue({ data: { run_id: "run-9", state: "queued" } });
      renderTab();

      fireEvent.click(await screen.findByTestId("preview-row-feat/x-runs-toggle"));
      expect(await screen.findByTestId("preview-run-form")).toBeInTheDocument();
      expect(await screen.findByTestId("preview-run-run-1")).toHaveTextContent("2 writes held");

      fireEvent.change(screen.getByTestId("preview-run-ref"), {
        target: { value: "workflows/new.procedure.yml" }
      });
      fireEvent.click(screen.getByTestId("preview-run-submit"));

      await waitFor(() =>
        expect(mocks.post).toHaveBeenCalledWith(`${LIST}/runs`, {
          branch: "feat/x",
          kind: "procedure",
          ref: "workflows/new.procedure.yml",
          variables: undefined,
          read_live_only: undefined
        })
      );
    });
  });

  it("renders nothing for someone who is not Oxy staff", () => {
    mocks.staff = false;
    serve([row({ branch: "feat/b" })]);
    renderTab();
    expect(screen.queryByTestId("settings-previews")).toBeNull();
    expect(screen.queryByTestId("new-preview-form")).toBeNull();
    expect(mocks.get).not.toHaveBeenCalled();
  });
});
