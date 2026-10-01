// @vitest-environment jsdom

/**
 * The Checks panel embedded in a preview row: the cheap `checks` summary
 * drives the label at once, and only expanding it fetches the per-pipeline
 * detail (`GET …/previews/checks`) — never eagerly, since most rows are
 * never opened.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PreviewChecksResponse, PreviewChecksSummary } from "@/types/workspace";

const mocks = vi.hoisted(() => ({ get: vi.fn() }));
vi.mock("@/services/api/axios", () => ({ apiClient: { get: mocks.get } }));

import PreviewChecksPanel from "./index";

const CHECKS_PATH = "/ws-1/previews/checks";

function serve(...responses: PreviewChecksResponse[]) {
  let call = 0;
  mocks.get.mockImplementation(() => {
    const data = responses[Math.min(call, responses.length - 1)];
    call += 1;
    return Promise.resolve({ data });
  });
}

const detail = (over: Partial<PreviewChecksResponse>): PreviewChecksResponse => ({
  branch: "feat/x",
  revision_id: "rev-1",
  status: "done",
  error: null,
  pipelines: [],
  transforms: [],
  ...over
});

let client: QueryClient;

function renderPanel(summary: PreviewChecksSummary | null) {
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    createElement(
      QueryClientProvider,
      { client },
      createElement(PreviewChecksPanel, {
        workspaceId: "ws-1",
        branch: "feat/x",
        summary,
        testId: "preview-row-feat/x"
      })
    )
  );
}

describe("PreviewChecksPanel", () => {
  beforeEach(() => {
    mocks.get.mockReset();
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("shows 'checking…' and stays inert when no analyze run exists yet", () => {
    renderPanel(null);
    const toggle = screen.getByTestId("preview-row-feat/x-checks-toggle");
    expect(toggle).toHaveTextContent("checking…");
    expect(toggle).toBeDisabled();
    expect(mocks.get).not.toHaveBeenCalled();
  });

  it("shows 'checking…' while the analysis itself is pending", () => {
    renderPanel({ status: "pending", needs_reset: 0, warnings: 0, transforms: 0 });
    expect(screen.getByTestId("preview-row-feat/x-checks-toggle")).toHaveTextContent("checking…");
  });

  it("shows 'no pipeline changes' when done and clean", () => {
    renderPanel({ status: "done", needs_reset: 0, warnings: 0, transforms: 0 });
    expect(screen.getByTestId("preview-row-feat/x-checks-toggle")).toHaveTextContent(
      "no pipeline changes"
    );
  });

  it("shows the reset/warning counts and expands to per-pipeline findings, naming the prod action", async () => {
    serve(
      detail({
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
        ]
      })
    );
    renderPanel({ status: "done", needs_reset: 1, warnings: 0, transforms: 0 });

    const toggle = screen.getByTestId("preview-row-feat/x-checks-toggle");
    expect(toggle).toHaveTextContent("1 need reset");
    expect(mocks.get).not.toHaveBeenCalled();

    fireEvent.click(toggle);

    await waitFor(() =>
      expect(mocks.get).toHaveBeenCalledWith(CHECKS_PATH, { params: { branch: "feat/x" } })
    );
    expect(await screen.findByTestId("preview-pipeline-check-toast")).toHaveTextContent(
      "orders: append → merge"
    );
    expect(screen.getByText(/Prod action: Reset schema, then backfill/)).toBeInTheDocument();
  });

  it("shows an additive finding's detail without a prod action line when it has none", async () => {
    serve(
      detail({
        pipelines: [
          {
            name: "orders",
            file_path: "airway/orders.airway.yml",
            change: "modified",
            verdict: "additive",
            findings: [
              {
                kind: "ColumnAdded",
                verdict: "additive",
                detail: "orders: added column 'tip_amount'",
                prod_action: null
              }
            ]
          }
        ]
      })
    );
    renderPanel({ status: "done", needs_reset: 0, warnings: 0, transforms: 0 });

    fireEvent.click(screen.getByTestId("preview-row-feat/x-checks-toggle"));

    expect(await screen.findByTestId("preview-pipeline-check-orders")).toHaveTextContent(
      "orders: added column 'tip_amount'"
    );
    expect(screen.queryByText(/Prod action/)).toBeNull();
  });

  it("polls the detail while it answers pending, and stops once it settles", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    serve(detail({ status: "pending", pipelines: [] }), detail({ status: "done", pipelines: [] }));
    renderPanel({ status: "done", needs_reset: 0, warnings: 1, transforms: 0 });

    fireEvent.click(screen.getByTestId("preview-row-feat/x-checks-toggle"));
    await waitFor(() => expect(mocks.get).toHaveBeenCalledTimes(1));

    await act(() => vi.advanceTimersByTimeAsync(2_000));
    await waitFor(() => expect(mocks.get).toHaveBeenCalledTimes(2));

    await act(() => vi.advanceTimersByTimeAsync(2_000 * 5));
    expect(mocks.get).toHaveBeenCalledTimes(2);
  });

  it("surfaces the analyzer's own error when the check failed", async () => {
    serve(
      detail({ status: "failed", error: "airway/toast.airway.yml: parse error", pipelines: [] })
    );
    renderPanel({ status: "failed", needs_reset: 0, warnings: 0, transforms: 0 });

    fireEvent.click(screen.getByTestId("preview-row-feat/x-checks-toggle"));
    expect(await screen.findByText("airway/toast.airway.yml: parse error")).toBeInTheDocument();
  });

  // Phase 2b (S10): transform builds share the same `checks` fetch as pipelines.
  describe("Transforms (2b)", () => {
    it("lists an auto transform with its build-run link, and a manual one with its reason", async () => {
      serve(
        detail({
          transforms: [
            {
              name: "je",
              file_path: "workflows/je.procedure.yml",
              change: "modified",
              build: "auto",
              reason: null,
              build_run_id: "build-1"
            },
            {
              name: "reconcile",
              file_path: "workflows/reconcile.procedure.yml",
              change: "added",
              build: "manual",
              reason: "needs variables: date (a, b)",
              build_run_id: null
            }
          ]
        })
      );
      renderPanel({ status: "done", needs_reset: 0, warnings: 0, transforms: 0 });

      fireEvent.click(screen.getByTestId("preview-row-feat/x-checks-toggle"));

      const auto = await screen.findByTestId("preview-transform-check-je");
      expect(auto).toHaveTextContent("Modified");
      expect(auto).toHaveTextContent("Auto build");
      expect(within(auto).getByTestId("preview-transform-check-je-build-link")).toBeInTheDocument();

      const manual = screen.getByTestId("preview-transform-check-reconcile");
      expect(manual).toHaveTextContent("Manual");
      expect(manual).toHaveTextContent("needs variables: date (a, b)");
      expect(
        within(manual).queryByTestId("preview-transform-check-reconcile-build-link")
      ).toBeNull();
    });

    it("opens the linked build run's detail from the Transforms section", async () => {
      // `serve()` above ignores the URL, fine while only one endpoint is hit;
      // the build-run link fetches a second, so route by path explicitly.
      mocks.get.mockImplementation((url: string) => {
        if (url === CHECKS_PATH) {
          return Promise.resolve({
            data: detail({
              transforms: [
                {
                  name: "je",
                  file_path: "workflows/je.procedure.yml",
                  change: "modified",
                  build: "auto",
                  reason: null,
                  build_run_id: "build-1"
                }
              ]
            })
          });
        }
        if (url === "/ws-1/previews/runs/build-1") {
          return Promise.resolve({
            data: {
              run_id: "build-1",
              branch: "feat/x",
              kind: "transform_build",
              target_ref: "workflows/je.procedure.yml",
              revision_id: "rev-1",
              state: "finished",
              outcome: "succeeded",
              held_count: 0,
              requested_by: null,
              created_at: "2026-09-29T08:00:00Z",
              started_at: null,
              finished_at: null,
              parent_run_id: "analyze-1",
              agentic_run_id: null,
              error: null,
              steps: [],
              compare: null
            }
          });
        }
        return Promise.resolve({ data: [] });
      });
      renderPanel({ status: "done", needs_reset: 0, warnings: 0, transforms: 0 });

      fireEvent.click(screen.getByTestId("preview-row-feat/x-checks-toggle"));
      fireEvent.click(await screen.findByTestId("preview-transform-check-je-build-link"));

      expect(
        await screen.findByTestId("preview-transform-check-je-build-dialog")
      ).toHaveTextContent("Build: je");
      expect(await screen.findByTestId("preview-run-detail-build-1")).toHaveTextContent(
        "No steps yet."
      );
    });
  });
});
