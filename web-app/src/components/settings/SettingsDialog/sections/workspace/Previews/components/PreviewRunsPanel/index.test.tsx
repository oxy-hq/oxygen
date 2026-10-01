// @vitest-environment jsdom

/**
 * The Runs panel: submitting a dry-run (`POST …/previews/runs`), the four
 * refusals the contract defines mapped to field text, and the runs list with
 * its held-step detail — driven down to what reaches `apiClient`, same style
 * as `Previews.test.tsx`.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { AxiosError, AxiosHeaders, type InternalAxiosRequestConfig } from "axios";
import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PreviewRunDetail, PreviewRunSummary } from "@/types/workspace";

const mocks = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn(), toastError: vi.fn() }));
vi.mock("@/services/api/axios", () => ({ apiClient: { get: mocks.get, post: mocks.post } }));
vi.mock("sonner", () => ({ toast: { error: mocks.toastError, success: vi.fn() } }));

import PreviewRunsPanel from "./index";

const RUNS_PATH = "/ws-1/previews/runs";
const AUTOMATIONS_PATH = "/ws-1/automations";

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

const runSummary = (over: Partial<PreviewRunSummary>): PreviewRunSummary => ({
  run_id: "run-1",
  branch: "feat/x",
  kind: "procedure",
  target_ref: "workflows/x.procedure.yml",
  revision_id: "rev-1",
  state: "finished",
  outcome: "succeeded",
  held_count: 3,
  requested_by: null,
  created_at: "2026-09-29T08:00:00Z",
  started_at: "2026-09-29T08:00:01Z",
  finished_at: "2026-09-29T08:00:05Z",
  parent_run_id: null,
  ...over
});

/** Route every GET by path/prefix; unmatched GETs (e.g. suggestions) answer empty. */
function serveGet(routes: Record<string, unknown>) {
  mocks.get.mockImplementation((url: string) => {
    for (const [path, data] of Object.entries(routes)) {
      if (url === path) return Promise.resolve({ data });
    }
    return Promise.resolve({ data: [] });
  });
}

let client: QueryClient;

function renderPanel() {
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    createElement(
      QueryClientProvider,
      { client },
      createElement(PreviewRunsPanel, { workspaceId: "ws-1", branch: "feat/x" })
    )
  );
}

describe("PreviewRunsPanel", () => {
  beforeEach(() => {
    mocks.get.mockReset();
    mocks.post.mockReset();
    mocks.toastError.mockReset();
    serveGet({ [RUNS_PATH]: [], [AUTOMATIONS_PATH]: [] });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("requires a procedure path before submitting", async () => {
    renderPanel();
    fireEvent.click(screen.getByTestId("preview-run-submit"));
    expect(await screen.findByTestId("preview-run-ref-error")).toHaveTextContent(
      "Enter a procedure path."
    );
    expect(mocks.post).not.toHaveBeenCalled();
  });

  it("rejects variables that aren't valid JSON, without submitting", async () => {
    renderPanel();
    fireEvent.change(screen.getByTestId("preview-run-ref"), {
      target: { value: "workflows/x.procedure.yml" }
    });
    fireEvent.change(screen.getByTestId("preview-run-variables"), {
      target: { value: "{not json" }
    });
    fireEvent.click(screen.getByTestId("preview-run-submit"));

    expect(await screen.findByTestId("preview-run-variables-error")).toHaveTextContent(
      "Variables must be valid JSON."
    );
    expect(mocks.post).not.toHaveBeenCalled();
  });

  it("starts a dry-run with the trimmed ref, parsed variables, and a fixed kind", async () => {
    mocks.post.mockResolvedValue({ data: { run_id: "run-9", state: "queued" } });
    renderPanel();

    fireEvent.change(screen.getByTestId("preview-run-ref"), {
      target: { value: "  workflows/compute_toast_journal_entry.procedure.yml  " }
    });
    fireEvent.change(screen.getByTestId("preview-run-variables"), {
      target: { value: '{"date": "2026-09-27"}' }
    });
    fireEvent.click(screen.getByTestId("preview-run-submit"));

    await waitFor(() =>
      expect(mocks.post).toHaveBeenCalledWith(RUNS_PATH, {
        branch: "feat/x",
        kind: "procedure",
        ref: "workflows/compute_toast_journal_entry.procedure.yml",
        variables: { date: "2026-09-27" },
        read_live_only: undefined
      })
    );
    // Cleared on success, and no generic toast for the happy path.
    await waitFor(() => expect(screen.getByTestId("preview-run-ref")).toHaveValue(""));
    expect(mocks.toastError).not.toHaveBeenCalled();
  });

  // Phase 2b (S8-full): opt a dry-run out of the read overlay.
  it("sends read_live_only: true only once the checkbox is checked", async () => {
    mocks.post.mockResolvedValue({ data: { run_id: "run-9", state: "queued" } });
    renderPanel();

    fireEvent.change(screen.getByTestId("preview-run-ref"), {
      target: { value: "workflows/x.procedure.yml" }
    });
    fireEvent.click(screen.getByTestId("preview-run-read-live-only"));
    fireEvent.click(screen.getByTestId("preview-run-submit"));

    await waitFor(() =>
      expect(mocks.post).toHaveBeenCalledWith(RUNS_PATH, {
        branch: "feat/x",
        kind: "procedure",
        ref: "workflows/x.procedure.yml",
        variables: undefined,
        read_live_only: true
      })
    );
    // Resets alongside the rest of the form on success.
    await waitFor(() =>
      expect(screen.getByTestId("preview-run-read-live-only")).toHaveAttribute(
        "data-state",
        "unchecked"
      )
    );
  });

  it.each([
    ["preview_runs_disabled", 404, "Preview runs aren't enabled on this deployment."],
    ["preview_not_ready", 409, "The branch is still compiling."],
    ["ref_not_in_revision", 404, "No procedure at that path on this branch."]
  ] as const)("maps %s to a field error, not a toast", async (code, status, message) => {
    mocks.post.mockRejectedValue(apiError(status, { code }));
    renderPanel();

    fireEvent.change(screen.getByTestId("preview-run-ref"), {
      target: { value: "workflows/x.yml" }
    });
    fireEvent.click(screen.getByTestId("preview-run-submit"));

    expect(await screen.findByTestId("preview-run-ref-error")).toHaveTextContent(message);
    expect(mocks.toastError).not.toHaveBeenCalled();
  });

  it("shows the runs list with each run's outcome and held-write count", async () => {
    serveGet({
      [RUNS_PATH]: [runSummary({ run_id: "run-1", held_count: 3, outcome: "succeeded" })],
      [AUTOMATIONS_PATH]: []
    });
    renderPanel();

    const row = await screen.findByTestId("preview-run-run-1");
    expect(row).toHaveTextContent("workflows/x.procedure.yml");
    expect(row).toHaveTextContent("3 writes held");
    expect(within(row).getByTestId("preview-run-state")).toHaveTextContent("Succeeded");
  });

  it("expands a run to its held steps, verb/targets/reason visible and SQL collapsed by default", async () => {
    const detail: PreviewRunDetail = {
      ...runSummary({}),
      agentic_run_id: "agentic-1",
      error: null,
      compare: null,
      sample: null,
      steps: [
        {
          name: "load_orders",
          kind: "execute_sql",
          status: "held",
          held: {
            verb: "INSERT",
            targets: ["analytics.journal"],
            reason: "Preview writes are held.",
            sql: "INSERT INTO analytics.journal ..."
          },
          redirected: null
        }
      ]
    };
    serveGet({
      [RUNS_PATH]: [runSummary({})],
      [`${RUNS_PATH}/run-1`]: detail,
      [AUTOMATIONS_PATH]: []
    });
    renderPanel();

    fireEvent.click(await screen.findByTestId("preview-run-run-1-toggle"));

    const step = await screen.findByTestId("preview-run-step-load_orders");
    expect(step).toHaveTextContent("INSERT");
    expect(step).toHaveTextContent("analytics.journal");
    expect(step).toHaveTextContent("Preview writes are held.");
    expect(screen.queryByText("INSERT INTO analytics.journal ...")).toBeNull();

    fireEvent.click(within(step).getByTestId("preview-run-step-load_orders-toggle-sql"));
    expect(screen.getByText("INSERT INTO analytics.journal ...")).toBeInTheDocument();
  });

  it("polls the runs list while a run hasn't finished, and stops once it has", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    serveGet({
      [RUNS_PATH]: [runSummary({ state: "running", outcome: null })],
      [AUTOMATIONS_PATH]: []
    });
    renderPanel();
    await screen.findByTestId("preview-run-run-1");
    const callsToRuns = () => mocks.get.mock.calls.filter(([url]) => url === RUNS_PATH).length;
    expect(callsToRuns()).toBe(1);

    serveGet({
      [RUNS_PATH]: [runSummary({ state: "finished", outcome: "succeeded" })],
      [AUTOMATIONS_PATH]: []
    });
    await act(() => vi.advanceTimersByTimeAsync(2_000));
    await waitFor(() => expect(callsToRuns()).toBe(2));

    await act(() => vi.advanceTimersByTimeAsync(2_000 * 5));
    expect(callsToRuns()).toBe(2);
  });

  // Phase 2b (S8-full): a redirected step succeeded, so it shows what went to
  // the preview's own copies instead of "would have written".
  it("expands a run to a redirected step's writes, reads and copies", async () => {
    const detail: PreviewRunDetail = {
      ...runSummary({}),
      agentic_run_id: "agentic-1",
      error: null,
      compare: null,
      sample: null,
      steps: [
        {
          name: "load_orders",
          kind: "execute_sql",
          status: "succeeded",
          held: null,
          redirected: {
            writes: [
              { live: "toast_pos.orders", preview: "preview_feat_x_92a1b7__toast_pos.orders" }
            ],
            reads: [{ live: "gl.daily", preview: "preview_feat_x_92a1b7__gl.daily" }],
            copies: [{ live: "toast_pos.orders", state: "shadow" }]
          }
        }
      ]
    };
    serveGet({
      [RUNS_PATH]: [runSummary({})],
      [`${RUNS_PATH}/run-1`]: detail,
      [AUTOMATIONS_PATH]: []
    });
    renderPanel();

    fireEvent.click(await screen.findByTestId("preview-run-run-1-toggle"));

    const step = await screen.findByTestId("preview-run-step-load_orders");
    expect(step).toHaveTextContent("toast_pos.orders → preview_feat_x_92a1b7__toast_pos.orders");
    expect(step).toHaveTextContent("gl.daily → preview_feat_x_92a1b7__gl.daily");
    expect(step).toHaveTextContent("toast_pos.orders (shadow copy)");
  });

  // Phase 2b (S10): a transform_build's compare nests under it in the list,
  // and the build's own detail shows the linked compare's caveats and tables.
  describe("Transform builds and compares (2b)", () => {
    const build = runSummary({
      run_id: "build-1",
      kind: "transform_build",
      target_ref: "workflows/je.procedure.yml",
      parent_run_id: "analyze-1",
      held_count: 0
    });
    const compareRun = runSummary({
      run_id: "compare-1",
      kind: "compare",
      target_ref: "workflows/je.procedure.yml",
      parent_run_id: "build-1",
      held_count: 0
    });
    const buildDetail: PreviewRunDetail = {
      ...build,
      agentic_run_id: null,
      error: null,
      steps: [],
      sample: null,
      compare: {
        run_id: "compare-1",
        state: "finished",
        outcome: "succeeded",
        error: null,
        diff_max_rows: 2_000_000,
        caveats: ["Live may be staler or fresher than the build."],
        tables: [
          {
            table: "toast_pos.sales_daily_metrics",
            live_rows: 1204,
            preview_rows: 1210,
            equal: false,
            only_in_preview: 6,
            only_in_live: 0,
            columns_added: ["top"],
            columns_removed: [],
            columns_retyped: [{ column: "order_count", live: "INTEGER", preview: "BIGINT" }],
            partial: false,
            dropped: false,
            preexisting: false,
            skipped_reason: null
          }
        ]
      }
    };

    it("nests the compare under its transform_build, and hides the (always-zero) held count on the compare", async () => {
      serveGet({
        [RUNS_PATH]: [compareRun, build],
        [AUTOMATIONS_PATH]: []
      });
      renderPanel();

      const list = await screen.findByTestId("preview-runs-list");
      // Only the build is a top-level entry — the compare nests inside it.
      expect(list.children).toHaveLength(1);

      const buildRow = within(list).getByTestId("preview-run-build-1");
      expect(buildRow).toHaveTextContent("Transform build");
      expect(buildRow).toHaveTextContent("0 writes held");

      const compareRow = within(buildRow).getByTestId("preview-run-compare-1");
      expect(compareRow).toHaveTextContent("Compare");
      expect(compareRow).not.toHaveTextContent("writes held");
    });

    it("shows the build's linked compare — caveats first, then per-table counts and badges", async () => {
      serveGet({
        [RUNS_PATH]: [build],
        [`${RUNS_PATH}/build-1`]: buildDetail,
        [AUTOMATIONS_PATH]: []
      });
      renderPanel();

      fireEvent.click(await screen.findByTestId("preview-run-build-1-toggle"));

      const caveats = await screen.findByTestId("preview-compare-caveats");
      expect(caveats).toHaveTextContent("Live may be staler or fresher than the build.");

      const table = screen.getByTestId("preview-compare-table-toast_pos.sales_daily_metrics");
      expect(table).toHaveTextContent("1,204");
      expect(table).toHaveTextContent("1,210");
      expect(table).toHaveTextContent("top");
      expect(table).toHaveTextContent("order_count (INTEGER → BIGINT)");
    });

    // Fix round 1: the build itself can finish (`state: "finished"`) while
    // its linked compare is still queued/running — without polling on that
    // too, the embedded compare view freezes on "Comparing…" forever.
    it("keeps polling a finished build's detail while its linked compare is still running, and stops once the compare finishes", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      const runningCompare: NonNullable<PreviewRunDetail["compare"]> = {
        run_id: "compare-1",
        state: "running",
        outcome: null,
        error: null,
        diff_max_rows: null,
        caveats: [],
        tables: []
      };
      let call = 0;
      mocks.get.mockImplementation((url: string) => {
        if (url === RUNS_PATH) return Promise.resolve({ data: [build] });
        if (url === `${RUNS_PATH}/build-1`) {
          call += 1;
          const compare =
            call === 1 ? runningCompare : { ...runningCompare, state: "finished" as const };
          return Promise.resolve({
            data: { ...build, agentic_run_id: null, error: null, steps: [], compare }
          });
        }
        return Promise.resolve({ data: [] });
      });
      renderPanel();

      fireEvent.click(await screen.findByTestId("preview-run-build-1-toggle"));
      await screen.findByTestId("preview-compare-compare-1");
      expect(call).toBe(1);

      await act(() => vi.advanceTimersByTimeAsync(2_000));
      await waitFor(() => expect(call).toBe(2));

      await act(() => vi.advanceTimersByTimeAsync(2_000 * 5));
      expect(call).toBe(2);
    });
  });

  // Phase 2b (S11): a bounded Airway sample, submitted next to the procedure
  // dry-run form above.
  describe("Sample an Airway pipeline (S11)", () => {
    it("requires a pipeline path before submitting", async () => {
      renderPanel();
      fireEvent.click(await screen.findByTestId("preview-sample-submit"));
      expect(await screen.findByTestId("preview-sample-ref-error")).toHaveTextContent(
        "Enter a pipeline path."
      );
      expect(mocks.post).not.toHaveBeenCalled();
    });

    it("starts a sample with the trimmed ref, an RFC-3339 [from, to) window, and parsed resources", async () => {
      mocks.post.mockResolvedValue({ data: { run_id: "sample-9", state: "queued" } });
      renderPanel();

      fireEvent.change(screen.getByTestId("preview-sample-ref"), {
        target: { value: "  pipelines/quickbooks_financials_eastbay.airway.yml  " }
      });
      fireEvent.change(screen.getByTestId("preview-sample-from"), {
        target: { value: "2026-09-20" }
      });
      fireEvent.change(screen.getByTestId("preview-sample-to"), {
        target: { value: "2026-09-26" }
      });
      fireEvent.change(screen.getByTestId("preview-sample-resources"), {
        target: { value: "accounts, journal_entries" }
      });
      fireEvent.click(screen.getByTestId("preview-sample-submit"));

      await waitFor(() =>
        expect(mocks.post).toHaveBeenCalledWith(RUNS_PATH, {
          branch: "feat/x",
          kind: "airway_sample",
          ref: "pipelines/quickbooks_financials_eastbay.airway.yml",
          window: { from: "2026-09-20T00:00:00.000Z", to: "2026-09-27T00:00:00.000Z" },
          resources: ["accounts", "journal_entries"]
        })
      );
      await waitFor(() => expect(screen.getByTestId("preview-sample-ref")).toHaveValue(""));
      expect(mocks.toastError).not.toHaveBeenCalled();
    });

    it("omits the window and resources when left blank", async () => {
      mocks.post.mockResolvedValue({ data: { run_id: "sample-9", state: "queued" } });
      renderPanel();

      fireEvent.change(screen.getByTestId("preview-sample-ref"), {
        target: { value: "pipelines/toast_sales.airway.yml" }
      });
      fireEvent.click(screen.getByTestId("preview-sample-submit"));

      await waitFor(() =>
        expect(mocks.post).toHaveBeenCalledWith(RUNS_PATH, {
          branch: "feat/x",
          kind: "airway_sample",
          ref: "pipelines/toast_sales.airway.yml",
          window: undefined,
          resources: undefined
        })
      );
    });

    it("refuses one bound without the other, without submitting", async () => {
      renderPanel();
      fireEvent.change(screen.getByTestId("preview-sample-ref"), {
        target: { value: "pipelines/toast_sales.airway.yml" }
      });
      fireEvent.change(screen.getByTestId("preview-sample-from"), {
        target: { value: "2026-09-20" }
      });
      fireEvent.click(screen.getByTestId("preview-sample-submit"));

      expect(await screen.findByTestId("preview-sample-to-error")).toHaveTextContent(
        "Enter both a start and an end, or leave both blank."
      );
      expect(mocks.post).not.toHaveBeenCalled();
    });

    it.each([
      ["sandbox_required", 409, "Register a sandbox source for this pipeline first."],
      ["window_required", 400, "Enter both a start and an end for the window, start before end."],
      ["window_too_long", 400, "The window can't be longer than 31 days."],
      [
        "window_not_supported",
        400,
        "This source has no date window — clear the window and try again."
      ],
      [
        "resources_required",
        400,
        "This source has more than one resource — choose which ones to sample."
      ],
      [
        "unknown_resource",
        400,
        "Unknown resource — check the name against what this source advertises."
      ],
      ["sample_refused", 422, "This source can't be sampled (replication slot or quota-limited)."],
      [
        "sample_unsupported",
        422,
        "This pipeline can't be sampled yet: Airway writes its own metadata tables in main, which a preview can't write."
      ]
    ] as const)("maps %s to a field error, not a toast", async (code, status, message) => {
      mocks.post.mockRejectedValue(apiError(status, { code }));
      renderPanel();

      fireEvent.change(screen.getByTestId("preview-sample-ref"), {
        target: { value: "pipelines/quickbooks_financials_eastbay.airway.yml" }
      });
      fireEvent.click(screen.getByTestId("preview-sample-submit"));

      expect(await screen.findByTestId("preview-sample-ref-error")).toHaveTextContent(message);
      expect(mocks.toastError).not.toHaveBeenCalled();
    });

    it("shows an airway_sample run labeled, with no held-write count, and expands to its schema compare", async () => {
      const sampleRun = runSummary({
        run_id: "sample-1",
        kind: "airway_sample",
        target_ref: "pipelines/quickbooks_financials_eastbay.airway.yml",
        held_count: 0
      });
      const sampleDetail: PreviewRunDetail = {
        ...sampleRun,
        agentic_run_id: null,
        error: null,
        compare: null,
        steps: [],
        sample: {
          pipeline: "quickbooks_financials_eastbay",
          dataset: "quickbooks_eastbay",
          window: { from: "2026-09-20T00:00:00Z", to: "2026-09-27T00:00:00Z" },
          resources: ["accounts"],
          wall_clock_capped: false,
          preview_pipeline: "preview:feat_qb_v2_92a1b7:quickbooks_financials_eastbay",
          tables: ["accounts", "journal_entries"],
          compared_with_live: true,
          verdict: "additive",
          findings: [
            { kind: "ColumnAdded", verdict: "additive", detail: "accounts.note", prod_action: null }
          ],
          partial: false,
          partial_reason: null,
          record_error: null
        }
      };
      serveGet({
        [RUNS_PATH]: [sampleRun],
        [`${RUNS_PATH}/sample-1`]: sampleDetail,
        [AUTOMATIONS_PATH]: []
      });
      renderPanel();

      const row = await screen.findByTestId("preview-run-sample-1");
      expect(row).toHaveTextContent("Airway sample");
      expect(row).not.toHaveTextContent("writes held");

      fireEvent.click(within(row).getByTestId("preview-run-sample-1-toggle"));

      const sample = await screen.findByTestId("preview-sample");
      expect(sample).toHaveTextContent("quickbooks_financials_eastbay");
      expect(sample).toHaveTextContent("dataset quickbooks_eastbay");
      expect(sample).toHaveTextContent("resources: accounts");
      expect(sample).toHaveTextContent("preview:feat_qb_v2_92a1b7:quickbooks_financials_eastbay");
      expect(sample).toHaveTextContent("accounts.note");
    });
  });
});
