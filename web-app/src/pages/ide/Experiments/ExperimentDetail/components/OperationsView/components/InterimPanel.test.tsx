// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  DailySeries,
  InterimOutcome,
  InterimResponse,
  ReportView,
  RunDetail
} from "@/types/experiment";
import InterimPanel from "./InterimPanel";
import RunBody from "./RunBody";

const state = vi.hoisted(() => ({
  query: {} as { data: InterimOutcome | undefined; isPending: boolean; error: Error | null },
  request: { mutate: vi.fn(), isPending: false, error: null as Error | null }
}));

vi.mock("@/hooks/api/useExperiments", () => ({
  useExperimentInterim: () => state.query,
  useRequestInterim: () => state.request
}));
vi.mock("@/components/Echarts", () => ({ Echarts: () => null }));
vi.mock("@/components/Echarts/resolveColor", () => ({
  resolveColor: () => "#000",
  resolveColorWithAlpha: () => "rgba(0,0,0,0.1)"
}));

const compliance = {
  c_treated: 0.9,
  c_control_leak: 0,
  n_treated: 6,
  n_control: 12,
  net: 0.9,
  floor: 0.8,
  below_floor: false,
  restated_mde: null,
  warning: null,
  restated: null
};

const report = (over: Partial<ReportView> = {}): ReportView =>
  ({
    status: { kind: "interim", through: "2026-03-09" },
    label: "INTERIM — not the final result (read through 2026-03-09)",
    experiment: "beverage_promo_pilot",
    hypothesis: "A promo lifts beverage revenue",
    target: "store_days.beverages_revenue",
    unit: "store",
    design: "units",
    estimand: "intention-to-treat effect per unit-day",
    refusal: null,
    directional: false,
    alpha: 0.05,
    assumptions: [],
    headline: {
      estimate: 18.2,
      half_width: 6.1,
      interval: { low: 12.1, high: 24.3 },
      p_value: 0.0004,
      significant: true,
      verdict: "significant at α = 0.05",
      text: "gross +18.20/unit-day (±6.10)"
    },
    treated_total: null,
    projection: null,
    substitution: null,
    direction: { predicted: "increase", observed: "increase", mismatch: false, message: null },
    promise: null,
    compliance,
    checks: { pre_trend: null, pre_trend_note: null, dropped_waves: [] },
    proposals: { count: 0, message: null },
    ratio: null,
    prediction: null,
    prediction_note: null,
    write_back: null,
    lever_note: null,
    seeds: { effect: 1, placebo: 2, ratio: 3 },
    ...over
  }) as unknown as ReportView;

const daily: DailySeries = {
  measure: "store_days.beverages_revenue",
  measured_until: null,
  design: "switchback",
  fleet_units: 4,
  washout_days: 1,
  periods: [{ from: "2026-04-01", to: "2026-04-08", on: true }],
  days: [{ date: "2026-04-01", fleet: 12.5, on: true, washout: true }]
};

const interim = (over: Partial<InterimResponse> = {}, reportOver: Partial<ReportView> = {}) =>
  ({
    experiment: "beverage_promo_pilot",
    run_id: "run-1",
    state: "live",
    design_id: "3f2a9c1e-0000-0000-0000-000000000001",
    running: false,
    requested_at: null,
    error: null,
    reading: {
      computed_at: "2026-03-10T04:30:00Z",
      through: "2026-03-09",
      report: report(reportOver),
      effect: { estimate: 999.9 },
      compliance,
      series: daily,
      alpha: 0.05,
      directional: false
    },
    ...over
  }) as InterimResponse;

const serve = (data: InterimResponse | undefined) => {
  state.query = {
    data: data && { kind: "interim", interim: data },
    isPending: false,
    error: null
  };
};

beforeEach(() => {
  state.request = { mutate: vi.fn(), isPending: false, error: null };
  serve(interim());
});
afterEach(cleanup);

const runDetail = (over: Partial<RunDetail> = {}) =>
  ({
    state: "live",
    design: "waves",
    periods: null,
    assign_by: null,
    group_membership: null,
    design_id: "d",
    design_version: 2,
    registered_by: "u",
    registered_at: "2026-02-20T03:15:00Z",
    retrospective: false,
    next_switch_on: null,
    flips: { pending: 0, sent: 0, confirmed: 0, unconfirmed: 0, failed: 0, cancelled: 0 },
    failed_flips: [],
    compliance: null,
    coverage: { last_window: null, floor: 0.9 },
    guardrails: [],
    guardrail_breach: null,
    abort_reason: null,
    checked_at: null,
    problems: [],
    next_check_at: "2026-03-10T01:00:00Z",
    not_advancing: false,
    estimate: { state: "none" },
    ...over
  }) as RunDetail;

describe("InterimPanel gating in RunBody", () => {
  it.each(["live", "closing"] as const)("is shown for a %s run", (s) => {
    render(<RunBody name='x' run={runDetail({ state: s })} leverRevisions={[]} />);
    expect(screen.getByTestId("experiment-interim")).toBeInTheDocument();
  });

  it("is hidden for a registered run", () => {
    render(<RunBody name='x' run={runDetail({ state: "registered" })} leverRevisions={[]} />);
    expect(screen.queryByTestId("experiment-interim")).toBeNull();
  });

  it("is hidden for a retrospective run", () => {
    render(<RunBody name='x' run={runDetail({ retrospective: true })} leverRevisions={[]} />);
    expect(screen.queryByTestId("experiment-interim")).toBeNull();
  });
});

describe("InterimPanel", () => {
  it("marks the reading interim, not final, with the day it was read through", () => {
    render(<InterimPanel name='x' />);
    expect(screen.getByTestId("experiment-interim-badge")).toHaveTextContent("Interim, not final");
    expect(screen.getByTestId("experiment-interim-badge")).toHaveAttribute("data-tone", "warning");
    expect(screen.getByTestId("experiment-interim-meta")).toHaveTextContent(
      "Read through 2026-03-09"
    );
    expect(screen.getByTestId("experiment-interim-meta")).toHaveTextContent("2026-03-10 04:30 UTC");
    expect(screen.getByTestId("experiment-interim-caveat-hint")).toBeInTheDocument();
  });

  it("words the verdict as so far, never as a final verdict", () => {
    render(<InterimPanel name='x' />);
    expect(screen.getByTestId("experiment-report-verdict-badge")).toHaveTextContent(
      "Looks significant so far"
    );
    cleanup();
    serve(interim({}, { headline: { ...report().headline!, significant: false } }));
    render(<InterimPanel name='x' />);
    expect(screen.getByTestId("experiment-report-verdict-badge")).toHaveTextContent(
      "Not distinguishable so far"
    );
  });

  it("says read through, not reported, in the pinned line", async () => {
    render(<InterimPanel name='x' />);
    await userEvent.click(screen.getByText(/how this was calculated/i));
    const pinned = screen.getByText(/^Design/, { selector: "p" });
    expect(pinned).toHaveTextContent("read through 2026-03-09.");
    expect(pinned).not.toHaveTextContent(/reported/i);
  });

  it("tiles say so far and how far the reading goes, without truncating the verdict", () => {
    render(<InterimPanel name='x' />);
    expect(screen.getByTestId("experiment-report-read-through")).toHaveTextContent("2026-03-09");
    expect(screen.getByTestId("experiment-report-stats")).toHaveTextContent(
      /(Passes|Misses) so far/
    );
  });

  it("shows the day-by-day series when present", () => {
    render(<InterimPanel name='x' />);
    expect(screen.getByTestId("experiment-report-series")).toBeInTheDocument();
  });

  it("renders no write-back, prediction or assumptions section", () => {
    render(<InterimPanel name='x' />);
    expect(screen.queryByTestId("experiment-write-back")).toBeNull();
    expect(screen.queryByTestId("experiment-prediction")).toBeNull();
    expect(screen.queryByText(/write back|apply/i)).toBeNull();
  });

  it("shows the empty state with Refresh when there is no reading yet", () => {
    serve(interim({ reading: null }));
    render(<InterimPanel name='x' />);
    expect(screen.getByTestId("experiment-interim-empty")).toHaveTextContent(
      "No interim yet. The first one is read once a whole post-window day has finished; it refreshes daily."
    );
    expect(screen.getByTestId("experiment-interim-refresh")).toBeEnabled();
  });

  it("disables Refresh and says Reading… while a reading is running", () => {
    serve(interim({ running: true }));
    render(<InterimPanel name='x' />);
    const button = screen.getByTestId("experiment-interim-refresh");
    expect(button).toBeDisabled();
    expect(button).toHaveTextContent("Reading…");
  });

  it("disables Refresh while the request is in flight", () => {
    state.request.isPending = true;
    render(<InterimPanel name='x' />);
    expect(screen.getByTestId("experiment-interim-refresh")).toBeDisabled();
  });

  it("calls the mutation when Refresh is clicked", async () => {
    render(<InterimPanel name='x' />);
    await userEvent.click(screen.getByTestId("experiment-interim-refresh"));
    expect(state.request.mutate).toHaveBeenCalledTimes(1);
  });

  it("shows the last attempt's error above the previous reading, in a non-destructive tone", () => {
    serve(interim({ error: "no wave has a whole post-window day yet: x" }));
    render(<InterimPanel name='x' />);
    const line = screen.getByTestId("experiment-interim-error");
    expect(line).toHaveTextContent("Last attempt: no wave has a whole post-window day yet: x");
    expect(line).not.toHaveAttribute("data-variant", "destructive");
    expect(screen.getByTestId("experiment-report-estimate")).toBeInTheDocument();
    expect(
      line.compareDocumentPosition(screen.getByTestId("experiment-report-estimate")) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy();
  });

  it("surfaces a refused request (e.g. 403) without crashing", () => {
    state.request.error = new Error("you need edit rights");
    render(<InterimPanel name='x' />);
    expect(screen.getByTestId("experiment-interim-request-error")).toHaveTextContent(
      "you need edit rights"
    );
  });

  it("renders a refusal verbatim with no estimate figures", () => {
    serve(interim({}, { refusal: "too few days to estimate", headline: null }));
    render(<InterimPanel name='x' />);
    expect(screen.getByTestId("experiment-report-refusal-text")).toHaveTextContent(
      "too few days to estimate"
    );
    expect(screen.queryByTestId("experiment-report-estimate")).toBeNull();
    expect(screen.queryByTestId("experiment-report-interval")).toBeNull();
  });

  it("never renders the audit-only raw effect", () => {
    render(<InterimPanel name='x' />);
    expect(document.body).not.toHaveTextContent("999.9");
  });
});
