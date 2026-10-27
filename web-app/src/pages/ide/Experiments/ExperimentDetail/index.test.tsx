// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  Design,
  ExperimentListItem,
  ExperimentStatus,
  ReportView,
  ResultOutcome,
  ResultResponse,
  RunStatus
} from "@/types/experiment";
import { definitionFixture } from "../fixtures";
import ExperimentDetailPage from "./index";

const h = vi.hoisted(() => ({
  list: { data: [] as ExperimentListItem[], isPending: false, error: null as Error | null },
  run: vi.fn(),
  result: vi.fn(),
  proposals: vi.fn(),
  price: vi.fn(),
  design: vi.fn(),
  adopt: vi.fn(),
  register: vi.fn(),
  write: vi.fn(),
  duplicate: vi.fn(),
  abort: vi.fn(),
  writeBack: vi.fn()
}));

vi.mock("@/hooks/api/useExperiments", () => ({
  useExperiments: () => ({ ...h.list, refetch: vi.fn() }),
  useExperimentRun: (...a: unknown[]) => h.run(...a),
  useExperimentResult: (...a: unknown[]) => h.result(...a),
  useExperimentProposals: (...a: unknown[]) => h.proposals(...a),
  usePriceExperiment: (...a: unknown[]) => h.price(...a),
  useExperimentDesign: (...a: unknown[]) => h.design(...a),
  useUseProposal: (...a: unknown[]) => h.adopt(...a),
  useRegisterExperiment: (...a: unknown[]) => h.register(...a),
  useSaveDesign: (...a: unknown[]) => h.write(...a),
  useDuplicateExperiment: (...a: unknown[]) => h.duplicate(...a),
  useAbortExperiment: (...a: unknown[]) => h.abort(...a),
  useExperimentInterim: () => ({ data: undefined, isPending: true, error: null }),
  useRequestInterim: () => ({ mutate: vi.fn(), isPending: false, error: null }),
  useApplyWriteBack: (...a: unknown[]) => h.writeBack(...a)
}));
vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "ws-1" }, branchName: "main" })
}));
vi.mock("@/stores/useIdeBranch", () => ({ default: () => ({ setCurrentBranch: vi.fn() }) }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

const item = (status: string, over: Partial<ExperimentListItem> = {}): ExperimentListItem => ({
  name: "beverage_promo_pilot",
  owner: "ops",
  hypothesis: "A promo lifts beverage revenue",
  design_id: "design-2",
  design_hash: "hash-2",
  design_frozen_at: null,
  status: status as ExperimentStatus,
  registration_refusal: null,
  next_switch_on: null,
  retrospective: false,
  retrospective_ready: false,
  proposals_count: 0,
  latest_proposal: null,
  definition: definitionFixture,
  ...over
});

const runStatus: RunStatus = {
  name: "beverage_promo_pilot",
  status: "live",
  registration_refusal: null,
  retrospective_ready: false,
  retrospective_refusal: null,
  lever_revisions: ["rev-a"],
  run: {
    state: "live",
    design: "waves",
    periods: null,
    assign_by: null,
    group_membership: null,
    design_id: "design-2",
    registered_by: "user-1",
    registered_at: "2026-02-20T03:15:00Z",
    retrospective: false,
    next_switch_on: "2026-03-16",
    flips: { pending: 0, sent: 0, confirmed: 0, unconfirmed: 0, failed: 0, cancelled: 0 },
    failed_flips: [],
    compliance: null,
    coverage: { last_window: null, floor: 0.9 },
    guardrails: [],
    guardrail_breach: null,
    abort_reason: null,
    abort_levers: null,
    aborted_on: null,
    units_on: [],
    checked_at: null,
    problems: [],
    next_check_at: "2026-03-10T01:00:00Z",
    not_advancing: false,
    estimate: { state: "none" }
  }
};

const report: ReportView = {
  status: { kind: "registered" },
  label: "Registered result",
  experiment: "beverage_promo_pilot",
  hypothesis: "h",
  target: "t",
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
    text: "gross +18.20/unit-day (±6.10); substitution not excluded above 86.00/unit-day"
  },
  treated_total: null,
  projection: null,
  substitution: {
    sibling: null,
    floor: null,
    statement: "No substitution check was registered.",
    sibling_effect: null,
    detected: false
  },
  direction: { predicted: "increase", observed: "increase", mismatch: false, message: null },
  promise: null,
  compliance: {
    c_treated: 1,
    c_control_leak: 0,
    n_treated: 6,
    n_control: 12,
    net: 1,
    floor: 0.8,
    below_floor: false,
    restated_mde: null,
    warning: null,
    restated: null
  },
  checks: { pre_trend: null, pre_trend_note: "No pre-period was long enough.", dropped_waves: [] },
  proposals: { count: 0, message: "0 assignments were drawn before the registered one." },
  ratio: null,
  prediction: null,
  prediction_note: null,
  write_back: null,
  lever_note: null,
  seeds: { effect: 1, placebo: 2, ratio: 3 }
};

const result: ResultResponse = {
  experiment: "beverage_promo_pilot",
  run_id: "run-1",
  state: "reported",
  design_id: "design-2",
  aborted: false,
  created_at: "2026-05-04T02:00:00Z",
  report,
  effect: {},
  ratio: null,
  compliance: {
    c_treated: 1,
    c_control_leak: 0,
    n_treated: 6,
    n_control: 12,
    restated_mde: null,
    below_floor: false
  },
  write_back: { available: false, reason: null, edit: null },
  directional: false,
  alpha: 0.05,
  prediction: null,
  assumptions: []
};

const resolved = (data: unknown) => ({ data, isPending: false, error: null, refetch: vi.fn() });

const mountAt = (name = "beverage_promo_pilot") =>
  render(
    <MemoryRouter initialEntries={[`/ide/experiments/${name}`]}>
      <Routes>
        <Route path='/ide/experiments/:name' element={<ExperimentDetailPage />} />
      </Routes>
    </MemoryRouter>
  );

const design: Design = {
  id: "design-2",
  content_hash: "hash-2",
  frozen_at: null,
  from_proposal_id: null,
  created_by: "user-1",
  created_at: "2026-02-18T03:15:00Z",
  updated_by: "user-1",
  updated_at: "2026-02-19T03:15:00Z",
  config: definitionFixture
};

const renderDetail = (status: string, over: Partial<ExperimentListItem> = {}) => {
  h.list = { data: [item(status, over)], isPending: false, error: null };
  return mountAt();
};

beforeEach(() => {
  h.run.mockReset().mockReturnValue(resolved(runStatus));
  h.result
    .mockReset()
    .mockReturnValue(resolved({ kind: "report", result } satisfies ResultOutcome));
  h.proposals.mockReset().mockReturnValue({ data: [], isSuccess: true });
  h.price.mockReset().mockReturnValue({ mutateAsync: vi.fn(), isPending: false });
  h.design.mockReset().mockReturnValue({ data: design, isError: false });
  h.duplicate.mockReset().mockReturnValue({ mutateAsync: vi.fn(), isPending: false });
  h.adopt.mockReset().mockReturnValue({ mutateAsync: vi.fn(), isPending: false });
  h.register.mockReset().mockReturnValue({ mutateAsync: vi.fn(), isPending: false });
  h.write.mockReset().mockReturnValue({ mutateAsync: vi.fn(), isPending: false });
  h.abort.mockReset().mockReturnValue({ mutateAsync: vi.fn(), isPending: false });
  h.writeBack.mockReset().mockReturnValue({ mutateAsync: vi.fn(), isPending: false });
  vi.mocked(toast.error).mockClear();
});
afterEach(cleanup);

describe("ExperimentDetailPage — status to view", () => {
  it.each(["draft", "live", "reported"])(
    "shows the whole definition for a %s experiment",
    (status) => {
      renderDetail(status);

      const def = screen.getByTestId("experiment-definition");
      expect(def).toHaveTextContent("fruit_summary");
      expect(def).toHaveTextContent("orders.total_order_value");
      expect(def).toHaveTextContent("orders.refund_rate");
    }
  );

  it("shows the pricing form for a draft, and asks for neither a run nor a result", () => {
    renderDetail("draft");

    expect(screen.getByTestId("experiment-detail")).toHaveAttribute("data-view", "pricing");
    expect(screen.getByTestId("experiment-price-form")).toBeInTheDocument();
    expect(h.run).not.toHaveBeenCalled();
    expect(h.result).not.toHaveBeenCalled();
  });

  it("offers to register a registrable experiment, and asks for neither a run nor a result", () => {
    renderDetail("registrable", { next_switch_on: "2026-03-02" });

    expect(screen.getByTestId("experiment-detail")).toHaveAttribute("data-view", "registrable");
    expect(screen.getByTestId("experiment-registrable")).toHaveTextContent("2026-03-02");
    expect(screen.getByTestId("experiment-registrable")).toHaveTextContent("Draft");
    expect(screen.getByTestId("experiment-register")).toBeInTheDocument();
    expect(h.run).not.toHaveBeenCalled();
    expect(h.result).not.toHaveBeenCalled();
  });

  it.each(["registered", "live", "closing"])(
    "shows operations only while %s, and never asks for a result",
    (status) => {
      renderDetail(status);

      expect(screen.getByTestId("experiment-detail")).toHaveAttribute("data-view", "operations");
      expect(screen.getByTestId("experiment-operations")).toBeInTheDocument();
      expect(screen.queryByTestId("experiment-report")).toBeNull();
      expect(h.result).not.toHaveBeenCalled();
    }
  );

  it.each(["reported", "aborted"])(
    "shows the report once %s, and does not ask for a run",
    (status) => {
      renderDetail(status);

      expect(screen.getByTestId("experiment-detail")).toHaveAttribute("data-view", "report");
      expect(screen.getByTestId("experiment-report-headline")).toHaveTextContent("gross +18.20");
      expect(h.run).not.toHaveBeenCalled();
    }
  );

  it("puts the report above the definition, so the result is the first thing read", () => {
    renderDetail("reported");

    const report = screen.getByTestId("experiment-report");
    const definition = screen.getByTestId("experiment-definition");
    expect(
      report.compareDocumentPosition(definition) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy();
  });

  it("shows a refused registration's message verbatim and asks for nothing", () => {
    renderDetail("registration_refused", {
      registration_refusal: "registered after the deadline: commit `abc123` <b>date</b>"
    });

    expect(screen.getByTestId("experiment-refused-text").textContent).toBe(
      "registered after the deadline: commit `abc123` <b>date</b>"
    );
    expect(h.run).not.toHaveBeenCalled();
    expect(h.result).not.toHaveBeenCalled();
  });

  it("shows a calm unknown-status view for a status this build does not know", () => {
    renderDetail("archived");

    expect(screen.getByTestId("experiment-unknown-state")).toHaveTextContent("archived");
    expect(h.run).not.toHaveBeenCalled();
    expect(h.result).not.toHaveBeenCalled();
  });

  it("fills the page width when the experiment is missing or still loading", () => {
    h.list = { data: [], isPending: false, error: null };
    mountAt("no_such_experiment");
    expect(screen.getByTestId("experiments-page")).toHaveClass("w-full", "flex-1");
    cleanup();

    h.list = { data: undefined as unknown as ExperimentListItem[], isPending: true, error: null };
    mountAt();
    expect(screen.getByTestId("experiments-page")).toHaveClass("w-full", "flex-1");
  });

  it("names the experiment, its status and owner in the header", () => {
    renderDetail("live");

    expect(screen.getByRole("heading", { name: "beverage_promo_pilot" })).toBeInTheDocument();
    expect(screen.getByTestId("experiment-state-badge")).toHaveTextContent("Live");
    expect(screen.getByText(/owner ops/)).toBeInTheDocument();
  });

  it("identifies the reported design by its short id, with no version", () => {
    renderDetail("reported");

    expect(screen.getByTestId("experiment-report")).toHaveTextContent("Design design-2");
    expect(screen.getByTestId("experiment-report")).not.toHaveTextContent(/version v\d/);
  });
});

describe("ExperimentDetailPage — result 409s", () => {
  it("renders the sealed state, not an error and not a toast", () => {
    h.result.mockReturnValue(
      resolved({ kind: "sealed", state: "reported" } satisfies ResultOutcome)
    );
    renderDetail("reported");

    expect(screen.getByTestId("experiment-sealed")).toHaveTextContent("The result is sealed");
    expect(screen.queryByTestId("experiment-report")).toBeNull();
    expect(screen.queryByText("Retry")).toBeNull();
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("renders a terminal run whose row is not written yet as 'being prepared', not as sealed", () => {
    h.result.mockReturnValue(
      resolved({ kind: "pending", state: "aborted" } satisfies ResultOutcome)
    );
    renderDetail("aborted");

    expect(screen.getByTestId("experiment-report-pending")).toHaveTextContent("being prepared");
    expect(screen.queryByTestId("experiment-sealed")).toBeNull();
    expect(screen.queryByText("Retry")).toBeNull();
    expect(toast.error).not.toHaveBeenCalled();
  });
});

describe("ExperimentDetailPage — not found and errors", () => {
  it("says the experiment was not found rather than rendering a blank page", () => {
    h.list = { data: [], isPending: false, error: null };
    mountAt("missing");

    expect(screen.getByText("Experiment not found")).toBeInTheDocument();
  });

  it("keeps the page mounted and shows an inline banner when a background refetch fails", () => {
    h.list = {
      data: [item("live")],
      isPending: false,
      error: new Error("not compiled yet; retry shortly")
    };
    h.run.mockReturnValue(resolved(runStatus));
    mountAt("beverage_promo_pilot");

    expect(screen.getByTestId("experiment-detail")).toBeInTheDocument();
    expect(screen.getByTestId("experiments-refresh-error")).toHaveTextContent(
      "not compiled yet; retry shortly"
    );
  });

  it("shows a failed list read as an error with a retry", () => {
    h.list = {
      data: undefined as unknown as ExperimentListItem[],
      isPending: false,
      error: new Error("not compiled yet; retry shortly")
    };
    mountAt("x");

    expect(screen.getByText("not compiled yet; retry shortly")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });
});
