// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DailySeries, ReportView, ResultResponse } from "@/types/experiment";
import ReportBody from "./ReportBody";

vi.mock("react-router-dom", () => ({ useNavigate: () => vi.fn() }));
vi.mock("@/hooks/api/useExperiments", () => ({
  useApplyWriteBack: () => ({ mutateAsync: vi.fn(), isPending: false })
}));
vi.mock("@/hooks/useRole", () => ({ useRole: () => ({ is: { workspaceEditor: true } }) }));
vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "ws-1" }, branchName: "main" })
}));
vi.mock("@/stores/useIdeBranch", () => ({ default: () => ({ setCurrentBranch: vi.fn() }) }));
vi.mock("@/components/Echarts", () => ({ Echarts: () => null }));
vi.mock("@/components/Echarts/resolveColor", () => ({
  resolveColor: () => "#000",
  resolveColorWithAlpha: () => "rgba(0,0,0,0.1)"
}));

afterEach(cleanup);

const report = (over: Partial<ReportView> = {}): ReportView => ({
  status: { kind: "registered" },
  label: "Registered result",
  experiment: "beverage_promo_pilot",
  hypothesis: "A beverage add-on promo lifts beverage revenue per store-day",
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
    text: "gross +18.20/unit-day (±6.10); substitution not excluded above 86.00/unit-day"
  },
  treated_total: {
    units: 3,
    days: 56,
    total: 3057.6,
    interval: { low: 2032.8, high: 4082.4 },
    basis: "Exact arithmetic on the treated set: mean × units × days."
  },
  projection: {
    label: "Projection to the 19 stores that were never treated",
    untreated_units: 19,
    days: 56,
    total: 19366.4,
    assumption: "Assumes the treated stores' mean effect carries to the rest.",
    warning: null
  },
  substitution: {
    sibling: "store_days.entree_revenue",
    floor: 86,
    statement: "Substitution is not excluded above 86.00/unit-day.",
    sibling_effect: null,
    detected: false
  },
  direction: { predicted: "increase", observed: "increase", mismatch: false, message: null },
  promise: {
    mde_at_proposal: 17.5,
    abs_estimate: 18.2,
    below_promise: false,
    message: "The estimate is above the 17.50/unit-day the design promised it could read."
  },
  compliance: {
    c_treated: 0.67,
    c_control_leak: 0,
    n_treated: 6,
    n_control: 12,
    net: 0.6123,
    floor: 0.8,
    below_floor: true,
    restated_mde: 26.1,
    warning:
      "Compliance is below the floor of 80%: this number measures the attempt, not the lever.",
    restated:
      "At 67% compliance this design could only have read a lever effect of about 26.10/unit-day."
  },
  checks: {
    pre_trend: { estimate: 0.4, t_stat: 0.8, diverged: false },
    pre_trend_note: "No divergence in the pre-period. A check, not proof.",
    dropped_waves: ["wave 3 dropped: its pre-window coverage was 0.71, below the floor of 0.9"]
  },
  proposals: { count: 2, message: "2 assignments were drawn before the registered one." },
  ratio: null,
  prediction: null,
  prediction_note: null,
  write_back: null,
  lever_note: null,
  seeds: { effect: 1, placebo: 2, ratio: 3 },
  ...over
});

const result = (
  reportOver: Partial<ReportView> = {},
  over: Partial<ResultResponse> = {}
): ResultResponse => {
  const r = report(reportOver);
  return {
    experiment: "beverage_promo_pilot",
    run_id: "run-1",
    state: "reported",
    design_id: "3f2a9c1e-0000-0000-0000-000000000001",
    aborted: false,
    created_at: "2026-05-04T02:00:00Z",
    report: r,
    effect: { estimate: 999.9 },
    ratio: null,
    compliance: {
      c_treated: 0.67,
      c_control_leak: 0,
      n_treated: 6,
      n_control: 12,
      restated_mde: 26.1,
      below_floor: true
    },
    write_back: { available: false, reason: null, edit: null },
    // Lifted from the report on the wire; the UI reads `report` and never these.
    directional: r.directional,
    alpha: r.alpha,
    prediction: r.prediction,
    assumptions: r.assumptions,
    ...over
  };
};

const daily: DailySeries = {
  measure: "store_days.beverages_revenue",
  measured_until: null,
  design: "switchback",
  fleet_units: 4,
  washout_days: 1,
  periods: [{ from: "2026-04-01", to: "2026-04-08", on: true }],
  days: [{ date: "2026-04-01", fleet: 12.5, on: true, washout: true }]
};

describe("ReportBody", () => {
  it("renders a result sealed before the series existed, without the day-by-day block", () => {
    render(<ReportBody result={result()} />);
    expect(screen.getByTestId("experiment-report-headline")).toBeInTheDocument();
    expect(screen.queryByTestId("experiment-report-series")).toBeNull();
    render(<ReportBody result={result({}, { series: null })} />);
    expect(screen.queryByTestId("experiment-report-series")).toBeNull();
  });

  it("renders the day-by-day series, even when the estimator refused", () => {
    render(<ReportBody result={result({}, { series: daily })} />);
    expect(screen.getByTestId("experiment-report-series")).toBeInTheDocument();
    cleanup();
    render(<ReportBody result={result({ refusal: "too few days" }, { series: daily })} />);
    expect(screen.getByTestId("experiment-report-refusal")).toBeInTheDocument();
    expect(screen.getByTestId("experiment-report-series")).toBeInTheDocument();
  });

  it("renders the report exactly as returned", () => {
    render(<ReportBody result={result()} />);

    expect(screen.getByTestId("experiment-report-label")).toHaveTextContent("Registered result");
    expect(screen.getByTestId("experiment-report-headline")).toHaveTextContent(
      "gross +18.20/unit-day (±6.10); substitution not excluded above 86.00/unit-day"
    );
    expect(screen.getByTestId("experiment-report-estimate")).toHaveTextContent("+18.20/unit-day");
    expect(screen.getByTestId("experiment-report-interval")).toHaveTextContent(
      "+12.10/unit-day to +24.30/unit-day"
    );
    expect(screen.getByTestId("experiment-report-substitution-floor")).toHaveTextContent(
      "Substitution is not excluded above 86.00/unit-day."
    );
    expect(screen.getByTestId("experiment-report-promise")).toHaveTextContent(
      "the design promised it could read"
    );
    const total = screen.getByTestId("experiment-report-treated-total");
    expect(total).toHaveTextContent("+3,057.60");
    expect(total).toHaveTextContent("+2,032.80 to +4,082.40");
    expect(screen.getByTestId("experiment-stat-treated-units")).toHaveTextContent("3");
    expect(screen.getByTestId("experiment-stat-window")).toHaveTextContent("56 days");
    expect(screen.getByTestId("experiment-report-dropped-waves")).toHaveTextContent(
      "wave 3 dropped: its pre-window coverage was 0.71, below the floor of 0.9"
    );
    expect(screen.getByTestId("experiment-report-pre-trend")).toHaveTextContent(
      "A check, not proof."
    );
    expect(screen.getByTestId("experiment-report-proposal-count")).toHaveTextContent(
      "2 assignments were drawn before the registered one."
    );
  });

  it("says substitution was detected when the sibling fell significantly, with no floor", () => {
    const detected = result({
      substitution: {
        sibling: "store_days.entree_revenue",
        floor: 86,
        statement:
          "substitution detected: store_days.entree_revenue fell 40.00 per unit-day (95% CI -45.00 to -35.00)",
        sibling_effect: {
          estimate: -40,
          interval: { low: -45, high: -35 },
          significant: true,
          refusal: null
        },
        detected: true
      }
    });
    render(<ReportBody result={detected} />);

    const row = screen.getByTestId("experiment-report-substitution-floor");
    expect(row).toHaveTextContent("Detected");
    expect(row).toHaveTextContent("substitution detected: store_days.entree_revenue fell 40.00");
    expect(row).not.toHaveTextContent("floor");
  });

  it("never renders a net figure, and never the audit-only raw effect", () => {
    const hostile = {
      ...result(),
      net_effect: 99.9,
      effect: { estimate: 999.9, net: 55.5 },
      report: { ...report(), net_of_substitution: 44.4 }
    } as unknown as ResultResponse;
    const { container } = render(<ReportBody result={hostile} />);

    const text = container.textContent ?? "";
    expect(text).not.toMatch(/\bnet\b/i);
    // compliance.net (0.6123) is on the wire and deliberately not shown.
    for (const leaked of ["99.9", "999.9", "55.5", "44.4", "0.6123", "61.2%"]) {
      expect(text).not.toContain(leaked);
    }
  });

  it("renders unbounded interval endpoints as unbounded, not as zero", () => {
    render(
      <ReportBody
        result={result({
          headline: {
            ...report().headline!,
            half_width: null,
            interval: { low: null, high: null },
            significant: false
          }
        })}
      />
    );

    const interval = screen.getByTestId("experiment-report-interval");
    expect(interval).toHaveTextContent("unbounded below to unbounded above");
    expect(interval.textContent).not.toContain("0.00");
  });

  it("renders a one-sided unbounded interval with its finite side intact", () => {
    render(
      <ReportBody
        result={result({
          headline: { ...report().headline!, interval: { low: 4.2, high: null } },
          treated_total: { ...report().treated_total!, interval: { low: null, high: 500 } }
        })}
      />
    );

    expect(screen.getByTestId("experiment-report-interval")).toHaveTextContent(
      "+4.20/unit-day to unbounded above"
    );
    expect(screen.getByTestId("experiment-report-treated-total")).toHaveTextContent(
      "unbounded below to +500.00"
    );
  });

  it("always labels the projection an assumption, whatever the server sent", () => {
    const { rerender } = render(
      <ReportBody
        result={result({ projection: { ...report().projection!, label: "", assumption: "" } })}
      />
    );
    const projection = within(screen.getByTestId("experiment-report-projection"));
    expect(projection.getByTestId("experiment-report-assumption")).toHaveTextContent("Assumption");
    expect(projection.getByText(/\+19,366\.40/)).toBeInTheDocument();

    rerender(
      <ReportBody
        result={result({
          projection: {
            ...report().projection!,
            warning: "Larger stores respond more; this likely overstates."
          }
        })}
      />
    );
    expect(screen.getByTestId("experiment-report-bias-warning")).toHaveTextContent(
      "likely overstates"
    );
  });

  it("renders an estimator refusal verbatim as the primary content", () => {
    const refusal = "every unit `store_*` is in one wave <script>x</script> * {{measure}} & more";
    const { container } = render(
      <ReportBody
        result={result({ refusal, headline: null, treated_total: null, projection: null })}
      />
    );

    expect(screen.getByTestId("experiment-report-refusal-text").textContent).toBe(refusal);
    expect(container.querySelector("script")).toBeNull();
    // A refused estimate is not shown as a number of any kind.
    expect(screen.queryByTestId("experiment-report-estimate")).toBeNull();
    expect(screen.queryByTestId("experiment-report-interval")).toBeNull();
  });

  it("labels an aborted run as aborted, with the server's label", () => {
    render(
      <ReportBody
        result={result(
          {
            status: { kind: "aborted", reason: "refunds spiked in wave 1" },
            label: "ABORTED RUN — not the registered result (refunds spiked in wave 1)"
          },
          { state: "aborted", aborted: true }
        )}
      />
    );

    expect(screen.getByTestId("experiment-report")).toHaveAttribute("data-report-state", "aborted");
    const band = screen.getByTestId("experiment-report-abort");
    expect(band).toHaveTextContent("Not the registered result (refunds spiked in wave 1)");
    expect(band).not.toHaveTextContent("ABORTED RUN —");
    expect(screen.getByTestId("experiment-abort-levers")).toHaveTextContent(
      "Levers were turned off"
    );
  });

  it("says when an aborted run left its levers as they were", () => {
    render(
      <ReportBody
        result={result(
          {
            status: { kind: "aborted", reason: "owner call", levers_kept: true },
            label: "ABORTED RUN — not the registered result (owner call)"
          },
          { state: "aborted", aborted: true }
        )}
      />
    );

    expect(screen.getByTestId("experiment-abort-levers")).toHaveTextContent(
      "Levers were left as they were"
    );
  });

  it("shows a retrospective report as a prominent warning with the server's label, without its shouted prefix", () => {
    render(
      <ReportBody
        result={result({
          status: { kind: "retrospective" },
          label: "RETROSPECTIVE — not pre-registered: exploratory analysis of past data"
        })}
      />
    );

    const band = screen.getByTestId("experiment-report-retrospective");
    expect(band).toHaveTextContent("Not pre-registered: exploratory analysis of past data");
    expect(band).not.toHaveTextContent("RETROSPECTIVE —");
    expect(screen.queryByTestId("experiment-report-abort")).toBeNull();
    expect(screen.queryByTestId("experiment-report-label")).toBeNull();
  });

  it("states the carryover the server measured, and nothing when the design read none", () => {
    const statement =
      "the effect lingered after the lever turned off: +9.50 per unit-day over the next 14 days (95% CI +5.50 to +13.50)";
    render(
      <ReportBody
        result={result({
          carryover: {
            days: 14,
            effect: {
              estimate: 9.5,
              interval: { low: 5.5, high: 13.5 },
              significant: true,
              refusal: null
            },
            lingered: true,
            statement
          }
        })}
      />
    );
    const row = screen.getByTestId("experiment-report-carryover");
    expect(row).toHaveTextContent("Lingered");
    expect(row).toHaveTextContent(statement);
    cleanup();
    render(<ReportBody result={result()} />);
    expect(screen.queryByTestId("experiment-report-carryover")).toBeNull();
  });

  it("warns below the compliance floor and restates the design's floor", () => {
    render(<ReportBody result={result()} />);

    expect(screen.getByTestId("experiment-stat-compliance")).toHaveTextContent("67.0%");
    const compliance = screen.getByTestId("experiment-report-compliance");
    expect(compliance).toHaveTextContent("measures the attempt, not the lever");
    expect(compliance).toHaveTextContent("about 26.10/unit-day");
  });

  it("says leakage was not measured when no control entry was read, not 0.0%", () => {
    render(
      <ReportBody
        result={result({
          compliance: { ...report().compliance, c_control_leak: 0, n_control: 0 }
        })}
      />
    );

    const tile = screen.getByTestId("experiment-stat-compliance");
    expect(tile).toHaveTextContent("control leakage not measured");
    expect(tile.textContent).not.toContain("leakage 0.0%");
  });

  it("says compliance was not measured when no treated entry was read, with no percentage and no warning", () => {
    render(
      <ReportBody
        result={result({
          compliance: {
            ...report().compliance,
            c_treated: 0,
            n_treated: 0,
            below_floor: true,
            restated_mde: 26.1
          }
        })}
      />
    );

    const tile = screen.getByTestId("experiment-stat-compliance");
    expect(tile).toHaveTextContent("Compliance not measured");
    expect(tile.textContent).not.toContain("0.0%");
    const page = screen.getByTestId("experiment-report").textContent ?? "";
    expect(page).not.toContain("measures the attempt");
    expect(page).not.toContain("could only have read");
  });

  it("uses the fixed warning when the server flags below-floor without a sentence", () => {
    render(
      <ReportBody
        result={result({ compliance: { ...report().compliance, warning: null, restated: null } })}
      />
    );

    expect(screen.getByTestId("experiment-report-compliance")).toHaveTextContent(
      "This number measures the attempt, not the lever."
    );
  });

  it("shows the direction-mismatch sentence only when the direction mismatched", () => {
    const { rerender } = render(<ReportBody result={result()} />);
    expect(screen.queryByTestId("experiment-report-direction-mismatch")).toBeNull();

    rerender(
      <ReportBody
        result={result({
          direction: {
            predicted: "increase",
            observed: "decrease",
            mismatch: true,
            message: "You predicted up; the estimate went down."
          }
        })}
      />
    );
    expect(screen.getByTestId("experiment-report-direction-mismatch")).toHaveTextContent(
      "You predicted up; the estimate went down."
    );
  });

  it("says when the lever's definition changed mid-run, and only then", () => {
    const { rerender } = render(<ReportBody result={result()} />);
    expect(screen.queryByTestId("experiment-lever-changed")).toBeNull();

    rerender(
      <ReportBody
        result={result({
          lever_note:
            "the lever's definition changed during the experiment: its flips ran 2 distinct revisions of the automation"
        })}
      />
    );
    expect(screen.getByTestId("experiment-lever-changed")).toHaveTextContent(
      "the lever's definition changed during the experiment"
    );
  });

  it("renders the server's verdict, never a label built from the significant boolean", () => {
    render(
      <ReportBody
        result={result({
          directional: true,
          alpha: 0.2,
          headline: { ...report().headline!, significant: true, verdict: "directional at α = 0.2" }
        })}
      />
    );

    expect(screen.getByTestId("experiment-report-verdict")).toHaveTextContent(
      "directional at α = 0.2"
    );
    // `significant: true` is on the wire; it must not surface as the word.
    expect(screen.getByTestId("experiment-report-effect").textContent).not.toMatch(
      /\bsignificant\b/i
    );
  });

  it("renders a verdict that missed the looser alpha exactly as the server worded it", () => {
    render(
      <ReportBody
        result={result({
          directional: true,
          alpha: 0.2,
          headline: {
            ...report().headline!,
            significant: false,
            verdict: "not distinguishable from zero at α = 0.2"
          }
        })}
      />
    );

    const verdict = screen.getByTestId("experiment-report-verdict");
    expect(verdict.textContent).toBe("not distinguishable from zero at α = 0.2");
    expect(verdict.textContent).not.toMatch(/directional/i);
  });

  it("states the estimand as the server worded it, and prices a groups design per group-day", () => {
    render(
      <ReportBody
        result={result({ design: "groups", estimand: "intention-to-treat effect per group-day" })}
      />
    );

    expect(screen.getByTestId("experiment-report-estimand")).toHaveTextContent(
      "intention-to-treat effect per group-day"
    );
    expect(screen.getByTestId("experiment-report-estimate")).toHaveTextContent("+18.20/group-day");
    expect(screen.getByTestId("experiment-report-interval")).toHaveTextContent(
      "+12.10/group-day to +24.30/group-day"
    );
    expect(screen.getByTestId("experiment-report-substitution-floor")).toHaveTextContent(
      "floor 86.00/group-day"
    );
  });

  it("shows the proposed slope and its edit when write-back is available", () => {
    render(
      <ReportBody
        result={result(
          {
            ratio: {
              coefficient: 1.42,
              interval: { low: 0.9, high: null },
              unbounded: false,
              first_stage: {
                estimate: 3,
                interval: { low: 1, high: 5 },
                significant: true,
                refusal: null
              },
              refusal: null
            }
          },
          {
            write_back: {
              available: true,
              reason: null,
              edit: {
                file_path: "sales_semantics/views/store_days.view.yml",
                target_measure: "store_days.beverages_revenue",
                driver_measure: "store_days.promo_redemptions",
                coefficient: 1.42,
                refs_entry: "experiments/beverage_promo_pilot",
                replaces: { value: 1.1, source: "fitted", edge_coefficient: 1.05 }
              }
            }
          }
        )}
      />
    );

    const section = screen.getByTestId("experiment-report-patch");
    expect(section).toHaveTextContent("store_days.promo_redemptions");
    expect(section).toHaveTextContent("store_days.beverages_revenue");
    expect(section).toHaveTextContent("1.42");
    expect(section).toHaveTextContent("0.9 to unbounded above");
    expect(section).toHaveTextContent("sales_semantics/views/store_days.view.yml");
    expect(screen.getByTestId("experiment-writeback-apply")).toBeInTheDocument();
    // The patch names the prior it would replace.
    expect(screen.getByTestId("experiment-report-patch-replaces")).toHaveTextContent(
      "1.1 (fitted from history); the edge's own coefficient is 1.05"
    );
  });

  it("says why nothing is proposed, verbatim, when write-back is unavailable", () => {
    render(
      <ReportBody
        result={result(
          {},
          {
            write_back: {
              available: false,
              reason: "the edge declares `form: log-log`",
              edit: null
            }
          }
        )}
      />
    );

    expect(screen.queryByTestId("experiment-report-patch")).toBeNull();
    expect(screen.getByTestId("experiment-report-no-writeback").textContent).toContain(
      "the edge declares `form: log-log`"
    );
  });

  it("shows no write-back section for an experiment with no driver", () => {
    render(<ReportBody result={result()} />);

    expect(screen.queryByTestId("experiment-report-patch")).toBeNull();
    expect(screen.queryByTestId("experiment-report-no-writeback")).toBeNull();
  });

  const PREDICTION: NonNullable<ReportView["prediction"]> = {
    predicted: 6.8,
    measured: 18.2,
    measured_interval: { low: 12.1, high: 24.3 },
    prior: { value: 1.7, source: "declared" },
    prior_in_interval: true,
    statement:
      "The experiment agrees with the World Model's slope (declared): the prior lies inside the interval the experiment earned for the coefficient."
  };

  it("sets the World Model's prediction against the measurement, with the prior's source", () => {
    render(<ReportBody result={result({ prediction: PREDICTION })} />);

    expect(screen.getByTestId("experiment-report-prior")).toHaveTextContent("1.7");
    expect(screen.getByTestId("experiment-report-prior")).toHaveTextContent(
      "declared in the World Model"
    );
    expect(screen.getByTestId("experiment-report-predicted")).toHaveTextContent("+6.80/unit-day");
    expect(screen.getByTestId("experiment-report-measured")).toHaveTextContent("+18.20/unit-day");
    expect(screen.getByTestId("experiment-report-measured")).toHaveTextContent(
      "+12.10/unit-day to +24.30/unit-day"
    );
    expect(screen.getByTestId("experiment-report-prediction-statement")).toHaveAttribute(
      "data-verdict",
      "agrees"
    );
  });

  it("renders the server's statement verbatim, never wording composed from the boolean", () => {
    const statement = "the slope held <b>inside</b> the earned set — see `outcome.driver` & more";
    const { container } = render(
      <ReportBody result={result({ prediction: { ...PREDICTION, statement } })} />
    );

    expect(screen.getByTestId("experiment-report-prediction-statement").textContent).toBe(
      statement
    );
    expect(container.querySelector("b")).toBeNull();
    // `prior_in_interval: true` must not add a sentence of the page's own.
    expect(screen.queryByText(/agrees with the World Model/i)).toBeNull();
  });

  it("marks a contradicted prior as contradicting, and says nothing of its own", () => {
    render(
      <ReportBody
        result={result({
          prediction: {
            ...PREDICTION,
            prior: { value: 4.2, source: "fitted" },
            prior_in_interval: false,
            statement: "The experiment contradicts the World Model's slope (fitted)."
          }
        })}
      />
    );

    const statement = screen.getByTestId("experiment-report-prediction-statement");
    expect(statement).toHaveAttribute("data-verdict", "contradicts");
    expect(statement.textContent).toBe(
      "The experiment contradicts the World Model's slope (fitted)."
    );
    expect(screen.getByTestId("experiment-report-prior")).toHaveTextContent("fitted from history");
  });

  it("says unknown, not agrees or contradicts, when prior_in_interval is null", () => {
    render(
      <ReportBody
        result={result({
          prediction: {
            ...PREDICTION,
            measured_interval: { low: 4.1, high: null },
            prior_in_interval: null,
            statement: "Neither confirmed nor contradicted: the earned set is unbounded above."
          }
        })}
      />
    );

    expect(screen.getByTestId("experiment-report-prediction-statement")).toHaveAttribute(
      "data-verdict",
      "unknown"
    );
    expect(screen.getByTestId("experiment-report-prediction-statement").textContent).toBe(
      "Neither confirmed nor contradicted: the earned set is unbounded above."
    );
  });

  it("renders the measured interval's null endpoints as unbounded, never as zero", () => {
    render(
      <ReportBody
        result={result({
          prediction: {
            ...PREDICTION,
            measured_interval: { low: null, high: null },
            prior_in_interval: null
          }
        })}
      />
    );

    const measured = screen.getByTestId("experiment-report-measured");
    expect(measured).toHaveTextContent("unbounded below to unbounded above");
    expect(measured.textContent).not.toContain("0.00");
  });

  it("prices the prediction in the estimand's unit: per group-day for a groups design", () => {
    render(<ReportBody result={result({ design: "groups", prediction: PREDICTION })} />);

    expect(screen.getByTestId("experiment-report-predicted")).toHaveTextContent("+6.80/group-day");
    expect(screen.getByTestId("experiment-report-measured")).toHaveTextContent(
      "+12.10/group-day to +24.30/group-day"
    );
  });

  it("shows why a frozen prior could not be compared, in the server's words, and no numbers", () => {
    render(
      <ReportBody
        result={result({
          prediction: null,
          prediction_note:
            "the run was aborted, so its truncated measurement is not a test of the slope"
        })}
      />
    );

    expect(screen.getByTestId("experiment-report-prediction-note")).toHaveTextContent(
      "the run was aborted, so its truncated measurement is not a test of the slope"
    );
    expect(screen.queryByTestId("experiment-report-predicted")).toBeNull();
    expect(screen.queryByTestId("experiment-report-prediction-statement")).toBeNull();
  });

  it("shows no prediction section when no slope was frozen", () => {
    render(<ReportBody result={result({ prediction: null, prediction_note: null })} />);

    expect(screen.queryByTestId("experiment-report-prediction")).toBeNull();
  });

  const SWITCHBACK_ASSUMPTIONS = [
    "Carryover longer than the washout leaks an on period's effect into the next off period, which biases the estimate toward zero.",
    "Pull-forward — a customer who buys during an on period instead of later depletes the next off period — biases the estimate away from zero."
  ];

  it("shows the switchback assumptions verbatim, beside a refusal too", () => {
    const { rerender } = render(
      <ReportBody result={result({ design: "switchback", assumptions: SWITCHBACK_ASSUMPTIONS })} />
    );

    const block = screen.getByTestId("experiment-report-assumptions");
    for (const sentence of SWITCHBACK_ASSUMPTIONS) expect(block).toHaveTextContent(sentence);
    // Un-collapsed: nothing to open to read them.
    expect(block.querySelector("details")).toBeNull();

    rerender(
      <ReportBody
        result={result({
          design: "switchback",
          assumptions: SWITCHBACK_ASSUMPTIONS,
          refusal: "fewer than 4 pairs retained",
          headline: null,
          treated_total: null,
          projection: null
        })}
      />
    );
    expect(screen.getByTestId("experiment-report-assumptions")).toHaveTextContent(
      SWITCHBACK_ASSUMPTIONS[1]
    );
  });

  it("shows no assumptions section for a design that states none", () => {
    render(<ReportBody result={result({ assumptions: [] })} />);

    expect(screen.queryByTestId("experiment-report-assumptions")).toBeNull();
  });
});

describe("ReportBody — at a glance", () => {
  it("leads with a summary card: the run's standing, the verdict and the effect in large type", () => {
    render(<ReportBody result={result()} />);

    const summary = screen.getByTestId("experiment-report-summary");
    const children = Array.from(screen.getByTestId("experiment-report").children);
    expect(children[0]).toBe(summary);
    expect(within(summary).getByTestId("experiment-report-label")).toHaveTextContent(
      "Registered result"
    );
    expect(within(summary).getByTestId("experiment-report-verdict-badge")).toHaveAttribute(
      "data-tone",
      "success"
    );
    expect(within(summary).getByTestId("experiment-report-estimate")).toHaveClass("text-4xl");
    expect(within(summary).getByTestId("experiment-report-interval")).toBeInTheDocument();
  });

  it("colours a directional verdict as a caution and a null result as neutral", () => {
    const { rerender } = render(
      <ReportBody
        result={result({
          directional: true,
          alpha: 0.2,
          headline: { ...report().headline!, verdict: "directional at α = 0.2" }
        })}
      />
    );
    expect(screen.getByTestId("experiment-report-verdict-badge")).toHaveAttribute(
      "data-tone",
      "warning"
    );

    rerender(
      <ReportBody
        result={result({
          headline: {
            ...report().headline!,
            significant: false,
            verdict: "not distinguishable from zero at α = 0.05"
          }
        })}
      />
    );
    expect(screen.getByTestId("experiment-report-verdict-badge")).toHaveAttribute(
      "data-tone",
      "neutral"
    );
  });

  it.each([
    [
      {},
      "Strong evidence",
      /95% confidence.*1 in 20 chance of a false alarm.*significant at α = 0\.05/
    ],
    [
      {
        directional: true,
        alpha: 0.2,
        headline: { ...report().headline!, verdict: "directional at α = 0.2" }
      },
      "Weak evidence",
      /80% confidence.*1 in 5 chance of a false alarm.*directional at α = 0\.2/
    ],
    [
      {
        headline: {
          ...report().headline!,
          significant: false,
          verdict: "not distinguishable from zero at α = 0.05"
        }
      },
      "No clear effect",
      /not distinguishable from zero at α = 0\.05/
    ]
  ] as const)(
    "words the verdict plainly, with α as confidence on hover (%#)",
    async (over, plain, hover) => {
      const user = userEvent.setup();
      render(<ReportBody result={result(over as Partial<ReportView>)} />);

      const badge = screen.getByTestId("experiment-report-verdict-badge");
      expect(badge.textContent).toBe(plain);
      await user.hover(badge);
      expect(await screen.findByRole("tooltip")).toHaveTextContent(hover);
    }
  );

  it("shows confidence instead of the p-value: whether the result clears the registered bar", () => {
    const { rerender } = render(<ReportBody result={result()} />);
    const tile = () => screen.getByTestId("experiment-stat-confidence");
    expect(tile()).toHaveTextContent("Passes the 95% bar");
    expect(tile()).toHaveAttribute("data-tone", "success");
    expect(tile()).toHaveTextContent("p = 0.000");
    expect(screen.queryByTestId("experiment-stat-p-value")).toBeNull();

    rerender(
      <ReportBody
        result={result({
          directional: true,
          alpha: 0.2,
          headline: { ...report().headline!, p_value: 0.12, verdict: "directional at α = 0.2" }
        })}
      />
    );
    expect(tile()).toHaveTextContent("Passes the 80% bar");
    expect(tile()).toHaveAttribute("data-tone", "warning");

    rerender(
      <ReportBody
        result={result({
          headline: { ...report().headline!, significant: false, p_value: 0.652 }
        })}
      />
    );
    expect(tile()).toHaveTextContent("Misses the 95% bar");
    expect(tile()).toHaveAttribute("data-tone", "neutral");
    expect(tile()).toHaveTextContent("p = 0.652");
  });

  it("shows the key numbers as stat tiles in plain labels", () => {
    render(<ReportBody result={result()} />);

    const tiles = within(screen.getByTestId("experiment-report-stats"));
    expect(tiles.getByTestId("experiment-stat-confidence")).toHaveTextContent("Passes the 95% bar");
    expect(tiles.getByTestId("experiment-report-treated-total")).toHaveTextContent("+3,057.60");
    expect(tiles.getByTestId("experiment-stat-treated-units")).toHaveTextContent("3");
    expect(tiles.getByTestId("experiment-report-projection")).toHaveTextContent("+19,366.40");
    expect(tiles.getByTestId("experiment-report-projection")).toHaveTextContent(
      "19 units × 56 days"
    );
    expect(tiles.getByTestId("experiment-stat-window")).toHaveTextContent("56 days");
    expect(tiles.getByTestId("experiment-stat-compliance")).toHaveTextContent("67.0%");
    expect(tiles.getByTestId("experiment-stat-compliance")).toHaveAttribute("data-tone", "warning");
  });

  it("keeps the totals in the summary card, with no separate total or projection sections", () => {
    render(
      <ReportBody
        result={result({
          projection: { ...report().projection!, warning: "This likely overstates." }
        })}
      />
    );

    const summary = within(screen.getByTestId("experiment-report-summary"));
    expect(summary.getByTestId("experiment-report-treated-total")).toBeInTheDocument();
    expect(summary.getByTestId("experiment-report-projection")).toBeInTheDocument();
    expect(summary.getByTestId("experiment-report-bias-warning")).toHaveTextContent(
      "This likely overstates."
    );
    // The basis and the projection's assumption move into the card's method notes.
    expect(summary.getByTestId("experiment-report-total-basis")).toHaveTextContent(
      "Exact arithmetic on the treated set"
    );
    expect(summary.getByTestId("experiment-report-projection-assumption")).toHaveTextContent(
      "Projection to the 19 stores that were never treated. Assumes the treated stores' mean effect carries to the rest."
    );
    expect(screen.queryByRole("heading", { name: "Total over the treated units" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Projection to untreated units" })).toBeNull();
  });

  it.each([
    ["experiment-stat-confidence", /chance of a false alarm.*not the chance the effect is real/i],
    ["experiment-report-treated-total", /every treated unit and every day/i],
    ["experiment-stat-treated-units", /switched on/i],
    ["experiment-report-projection", /never treated/i],
    ["experiment-stat-window", /days after the switch/i],
    ["experiment-stat-compliance", /change actually landed/i],
    ["experiment-report-estimate-hint", /average change in the target/i],
    ["experiment-report-interval-hint", /includes zero/i]
  ])("explains %s in plain words on hover", async (testId, text) => {
    const user = userEvent.setup();
    render(<ReportBody result={result()} />);

    await user.hover(screen.getByTestId(testId));
    expect(await screen.findByRole("tooltip")).toHaveTextContent(text);
  });

  it("has no compliance section and folds the secondary detail into one collapsed section", () => {
    render(
      <ReportBody
        result={result(
          {
            direction: {
              predicted: "increase",
              observed: "decrease",
              mismatch: true,
              message: "You predicted up; the estimate went down."
            }
          },
          { write_back: { available: false, reason: "no driver edge", edit: null } }
        )}
      />
    );

    expect(
      screen.queryByRole("heading", { name: "Compliance (every unit as assigned)" })
    ).toBeNull();
    const details = screen.getByTestId("experiment-report-details");
    const trigger = within(details).getByRole("button", { name: /details/i });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    for (const id of [
      "experiment-report-substitution-floor",
      "experiment-report-promise",
      "experiment-report-compliance",
      "experiment-report-pre-trend",
      "experiment-report-dropped-waves",
      "experiment-report-proposal-count",
      "experiment-report-no-writeback"
    ]) {
      expect(within(details).getByTestId(id)).toBeInTheDocument();
    }
    // A direction that contradicts the prediction stays in view, in the card.
    expect(
      within(screen.getByTestId("experiment-report-summary")).getByTestId(
        "experiment-report-direction-mismatch"
      )
    ).toHaveTextContent("You predicted up");
  });

  it("lays the details out as a checklist: a status per check, the server's sentence beneath", () => {
    render(
      <ReportBody
        result={result(
          {},
          { write_back: { available: false, reason: "no driver edge", edit: null } }
        )}
      />
    );

    const row = (id: string) => screen.getByTestId(`experiment-check-${id}`);
    expect(row("substitution")).toHaveAttribute("data-tone", "info");
    expect(row("promise")).toHaveAttribute("data-tone", "success");
    expect(row("promise")).toHaveTextContent("Clears the promised size");
    expect(row("compliance")).toHaveAttribute("data-tone", "warning");
    expect(row("pre-trend")).toHaveAttribute("data-tone", "success");
    expect(row("pre-trend")).toHaveTextContent("No divergence");
    expect(row("dropped")).toHaveAttribute("data-tone", "warning");
    expect(row("dropped")).toHaveTextContent("1 dropped");
    expect(row("randomisation")).toHaveTextContent("2 earlier draws");
    expect(row("semantic-model")).toHaveTextContent("Nothing proposed");
    // The collapsed header says how many need a look before it is opened.
    expect(screen.getByTestId("experiment-report-details-summary")).toHaveTextContent(
      "2 to review"
    );
  });

  it("flags compliance, direction, pre-trend and a changed lever as coloured badges", () => {
    render(
      <ReportBody
        result={result({
          direction: {
            predicted: "increase",
            observed: "decrease",
            mismatch: true,
            message: "You predicted up; the estimate went down."
          },
          checks: { ...report().checks, pre_trend: { estimate: 3, t_stat: 4, diverged: true } },
          lever_note: "the lever's definition changed during the experiment"
        })}
      />
    );

    const badges = within(screen.getByTestId("experiment-report-badges"));
    expect(badges.getByTestId("experiment-badge-compliance")).toHaveAttribute(
      "data-tone",
      "warning"
    );
    expect(badges.getByTestId("experiment-badge-direction")).toHaveAttribute("data-tone", "danger");
    expect(badges.getByTestId("experiment-badge-pre-trend")).toHaveAttribute(
      "data-tone",
      "warning"
    );
    expect(badges.getByTestId("experiment-badge-lever")).toHaveAttribute("data-tone", "warning");
  });

  it("marks aborted and retrospective runs with a coloured run badge", () => {
    const { rerender } = render(
      <ReportBody
        result={result(
          { status: { kind: "aborted", reason: "refunds spiked" }, label: "ABORTED RUN" },
          { state: "aborted", aborted: true }
        )}
      />
    );
    expect(screen.getByTestId("experiment-badge-run")).toHaveAttribute("data-tone", "danger");
    expect(screen.getByTestId("experiment-badge-run")).toHaveTextContent("Aborted run");

    rerender(
      <ReportBody result={result({ status: { kind: "retrospective" }, label: "RETROSPECTIVE" })} />
    );
    expect(screen.getByTestId("experiment-badge-run")).toHaveAttribute("data-tone", "warning");
    expect(screen.getByTestId("experiment-badge-run")).toHaveTextContent("Retrospective");
  });

  it("keeps the method prose in collapsed 'How this was calculated' sections", () => {
    render(<ReportBody result={result()} />);

    const triggers = screen.getAllByRole("button", { name: /how this was calculated/i });
    expect(triggers.length).toBeGreaterThan(0);
    for (const t of triggers) expect(t).toHaveAttribute("aria-expanded", "false");
    // Collapsed, not removed: the estimand is still on the page for whoever opens it.
    expect(screen.getByTestId("experiment-report-estimand")).toHaveTextContent(
      "intention-to-treat effect per unit-day"
    );
  });
});

describe("ReportBody on an observed or hand-assigned split", () => {
  it("says an observed split was declared, not randomised", () => {
    render(
      <ReportBody
        result={result({
          status: { kind: "retrospective" },
          proposals: {
            count: 0,
            message: "Observational: the assignment was declared, not randomised",
            observational: true,
            hand_assigned: false
          }
        })}
      />
    );
    const check = screen.getByTestId("experiment-report-proposal-count");
    expect(check).toHaveTextContent("Observational");
    expect(check).not.toHaveTextContent("Not priced by the platform");
  });

  it("says a hand-written split is not a platform draw", () => {
    render(
      <ReportBody
        result={result({
          proposals: {
            count: 1,
            message: "the analysed split was written by hand, not drawn by the platform",
            observational: false,
            hand_assigned: true
          }
        })}
      />
    );
    const check = screen.getByTestId("experiment-report-proposal-count");
    expect(check).toHaveTextContent("Written by hand");
    expect(check).not.toHaveTextContent("earlier draw");
  });

  it("states why no MDE was priced instead of a promise", () => {
    render(
      <ReportBody
        result={result({
          promise: null,
          mde_note: "MDE unavailable: no smallest detectable effect was priced for this design"
        })}
      />
    );
    expect(screen.getByTestId("experiment-report-promise")).toHaveTextContent("MDE unavailable");
  });
});

describe("ReportBody with a long headline", () => {
  it("clamps a headline naming hundreds of filter values, and opens it on request", () => {
    const skus = Array.from({ length: 200 }, (_, i) => `SKU-${i}`).join(" or ");
    render(
      <ReportBody
        result={result({
          headline: {
            ...report().headline!,
            text: `gross +2.79 per unit-day on sales.units (sku = ${skus})`
          }
        })}
      />
    );
    const headline = screen.getByTestId("experiment-report-headline");
    expect(headline).toHaveClass("line-clamp-3");
    fireEvent.click(screen.getByRole("button", { name: "Show all" }));
    expect(headline).not.toHaveClass("line-clamp-3");
    expect(screen.getByRole("button", { name: "Show less" })).toBeInTheDocument();
  });

  it("leaves a short headline whole, with no toggle", () => {
    render(<ReportBody result={result({})} />);
    expect(screen.getByTestId("experiment-report-headline")).not.toHaveClass("line-clamp-3");
    expect(screen.queryByRole("button", { name: "Show all" })).toBeNull();
  });
});

describe("ReportBody with units left out", () => {
  it("lists each unit the estimate dropped, with its reason", () => {
    render(
      <ReportBody
        result={result({
          checks: {
            ...report().checks,
            dropped_units: ["s99: a treated unit with no rows in the panel"]
          }
        })}
      />
    );
    const check = screen.getByTestId("experiment-report-dropped-waves");
    expect(check).toHaveTextContent("1 with no data");
    expect(check).toHaveTextContent("s99: a treated unit with no rows in the panel");
  });

  it("groups many units dropped for one reason into one line that names a few", () => {
    const missing = Array.from(
      { length: 40 },
      (_, i) => `c${i + 1}: a control unit with no rows in the panel`
    );
    render(
      <ReportBody
        result={result({
          checks: {
            ...report().checks,
            dropped_units: [
              "s7: holds 45 of the panel's 53 days, under the 90% a unit needs",
              ...missing
            ]
          }
        })}
      />
    );
    const check = screen.getByTestId("experiment-report-dropped-waves");
    expect(check).toHaveTextContent("1 unit dropped, 40 with no data");
    expect(check).toHaveTextContent(
      "s7: holds 45 of the panel's 53 days, under the 90% a unit needs"
    );
    expect(check).toHaveTextContent(
      "40 units: a control unit with no rows in the panel (c1, c2, c3 and 37 more)"
    );
    expect(check).not.toHaveTextContent("c40");
  });
});

describe("ReportBody for a ratio outcome", () => {
  it("shows the share change per unit, with no dollar sign", () => {
    render(
      <ReportBody
        result={result({
          share: true,
          headline: { ...report().headline!, estimate: 0.12 }
        })}
      />
    );
    const estimate = screen.getByTestId("experiment-report-estimate");
    expect(estimate).toHaveTextContent("+12.0 pp per unit");
    expect(estimate).not.toHaveTextContent("$");
  });
});

describe("ReportBody for a proportional outcome", () => {
  it("shows the lift as a percentage", () => {
    render(
      <ReportBody
        result={result({
          proportional: true,
          headline: {
            ...report().headline!,
            estimate: 0.5,
            interval: { low: 0.228, high: 0.832 }
          }
        })}
      />
    );
    expect(screen.getByTestId("experiment-report-estimate")).toHaveTextContent("+50.0%");
    expect(screen.getByTestId("experiment-report-interval")).toHaveTextContent("+22.8% to +83.2%");
  });

  it("says the day-by-day figures are the measure's own units, not percentages", () => {
    render(<ReportBody result={result({ proportional: true }, { series: daily })} />);
    expect(screen.getByTestId("experiment-report-series-units")).toHaveTextContent(
      "not percentages"
    );
    cleanup();
    render(<ReportBody result={result({}, { series: daily })} />);
    expect(screen.queryByTestId("experiment-report-series-units")).toBeNull();
  });
});

describe("ReportBody per-wave table", () => {
  const wave = (switch_on: string, estimate: number, significant: boolean) => ({
    switch_on,
    n_treated: 3,
    n_control: 160,
    estimate,
    interval: { low: estimate - 5, high: estimate + 5 },
    p_value: significant ? 0.01 : 0.4,
    significant,
    refusal: null
  });

  it("lists each wave with its own verdict when there are two or more", () => {
    render(
      <ReportBody
        result={result({
          waves: [wave("2026-05-06", 11, true), wave("2026-05-14", 0.4, false)]
        })}
      />
    );
    const rows = within(screen.getByTestId("experiment-report-waves")).getAllByTestId(
      "experiment-report-wave"
    );
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent("2026-05-06");
    expect(rows[0]).toHaveTextContent("Clear on its own");
    expect(rows[1]).toHaveTextContent("No clear effect");
  });

  it("shows no table for a single wave", () => {
    render(<ReportBody result={result({ waves: [wave("2026-05-06", 11, true)] })} />);
    expect(screen.queryByTestId("experiment-report-waves")).toBeNull();
  });
});

describe("ReportBody for a proportional outcome with a sibling", () => {
  it("keeps the sibling's floor in the measure's units", () => {
    render(<ReportBody result={result({ proportional: true })} />);
    const floor = screen.getByTestId("experiment-report-substitution-floor");
    expect(floor).toHaveTextContent("86.00/unit-day");
    expect(floor).not.toHaveTextContent("%");
  });
});
