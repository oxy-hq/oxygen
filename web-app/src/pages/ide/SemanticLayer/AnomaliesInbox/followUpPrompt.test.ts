import { describe, expect, it } from "vitest";
import type { MetricAnomaly } from "@/types/metricAnomalies";
import type { DriverAttribution, ExplainResult } from "@/types/metricTree";
import { buildFollowUpPrompt, warningMessage } from "./followUpPrompt";

function anomaly(overrides: Partial<MetricAnomaly> = {}): MetricAnomaly {
  return {
    id: "an-1",
    workspace_id: "ws-1",
    measure: "sales.net_sales",
    time_dimension: "sales.business_date",
    granularity: "day",
    period_start: "2026-07-21T00:00:00Z",
    period_end: "2026-07-22T00:00:00Z",
    observed: 512.31,
    expected: 1_101.7,
    lower_bound: 900,
    upper_bound: 1_300,
    z_score: -4.12,
    severity: "high",
    status: "new",
    label: "Net sales",
    dimension_key: "",
    filters: null,
    event_id: null,
    detected_at: "2026-07-22T02:00:00Z",
    updated_at: "2026-07-22T02:00:00Z",
    ...overrides
  };
}

function driver(overrides: Partial<DriverAttribution> = {}): DriverAttribution {
  return {
    driver_measure: "sales.total_discounts",
    driver_previous: 1_000,
    driver_current: 939.42,
    driver_delta: -60.58,
    direction: "negative",
    contribution: "counteracting",
    form: "linear",
    ...overrides
  };
}

function result(overrides: Partial<ExplainResult> = {}): ExplainResult {
  return {
    target: "sales.net_sales",
    target_delta: -589.39,
    target_previous: 1_101.7,
    target_current: 512.31,
    time_dimension: "sales.business_date",
    current_period: ["2026-07-21", "2026-07-21"],
    previous_period: ["2026-07-14", "2026-07-14"],
    nodes: [],
    coverage: 0.91,
    ...overrides
  };
}

/** The driver line for `measure`, without the leading bullet. */
function driverLine(prompt: string, measure: string): string {
  const line = prompt.split("\n").find((l) => l.trim().startsWith(`• ${measure}`));
  if (!line) throw new Error(`no driver line for ${measure} in:\n${prompt}`);
  return line.trim().slice(2);
}

describe("buildFollowUpPrompt", () => {
  it("fences the context and ends with the user's literal question", () => {
    const prompt = buildFollowUpPrompt(anomaly(), null, null, "Why did this drop?");
    expect(prompt.startsWith("```context\n")).toBe(true);
    expect(prompt.endsWith("```\n\nWhy did this drop?")).toBe(true);
  });

  it("tells the agent an offsetting driver did not cause the move", () => {
    const prompt = buildFollowUpPrompt(
      anomaly(),
      null,
      result({ driver_attribution: [driver()] }),
      "why?"
    );
    const line = driverLine(prompt, "sales.total_discounts");
    expect(line).toContain("moved AGAINST the anomaly");
    expect(line).toContain("did NOT cause it");
    expect(line).toContain("· negative relationship");
  });

  // The bug this test exists for: `direction` is optional so a cached explain
  // written before classification still deserializes, and interpolating it
  // directly put the literal string "undefined" in front of the model.
  it("never emits 'undefined' for a driver missing its direction", () => {
    const prompt = buildFollowUpPrompt(
      anomaly(),
      null,
      result({
        driver_attribution: [driver({ direction: undefined, contribution: undefined })]
      }),
      "why?"
    );
    expect(prompt).not.toContain("undefined");
    const line = driverLine(prompt, "sales.total_discounts");
    expect(line).not.toContain("relationship");
    expect(line).toContain("direction undetermined");
  });

  it("omits the relationship fragment when the direction is explicitly unknown", () => {
    const prompt = buildFollowUpPrompt(
      anomaly(),
      null,
      result({ driver_attribution: [driver({ direction: "unknown" })] }),
      "why?"
    );
    expect(driverLine(prompt, "sales.total_discounts")).not.toContain("unknown relationship");
  });

  it("quotes the rate-driven half of a mechanical driver and forbids citing it", () => {
    const prompt = buildFollowUpPrompt(
      anomaly(),
      null,
      result({
        driver_attribution: [
          driver({
            passthrough: {
              base_measure: "sales.total_gross_sales",
              ratio_previous: 0.0964,
              ratio_current: 0.1002,
              base_driven_delta: -62.68,
              ratio_driven_delta: 2.11
            }
          })
        ]
      }),
      "why?"
    );
    expect(driverLine(prompt, "sales.total_discounts")).toContain(
      "do not cite as a cause or an offset"
    );
    expect(prompt).toContain("MECHANICAL — tracks sales.total_gross_sales");
    expect(prompt).toContain("ratio 9.64% → 10.02%");
    expect(prompt).toContain("-62.68 is forced by the base and only +2.11 is the ratio itself");
  });

  it("says a qualitative driver has no magnitude instead of quoting one", () => {
    const prompt = buildFollowUpPrompt(
      anomaly(),
      null,
      result({
        driver_attribution: [
          driver({ contribution: "contributing", estimated_target_impact: undefined })
        ]
      }),
      "why?"
    );
    const line = driverLine(prompt, "sales.total_discounts");
    expect(line).toContain("(qualitative — no coefficient, so no magnitude)");
    expect(line).not.toContain("est. target impact");
  });

  it("keeps a sub-1 rate driver's movement visible in the context", () => {
    const prompt = buildFollowUpPrompt(
      anomaly(),
      null,
      result({
        driver_attribution: [
          driver({
            driver_measure: "sales.discount_rate",
            driver_previous: 0.0964,
            driver_current: 0.1002,
            driver_delta: 0.0038,
            contribution: "contributing"
          })
        ]
      }),
      "why?"
    );
    expect(driverLine(prompt, "sales.discount_rate")).toContain("Δ +0.00380 (0.0964 → 0.100)");
  });

  it("labels a description as the standing relationship, not this period's move", () => {
    const prompt = buildFollowUpPrompt(
      anomaly(),
      null,
      result({
        driver_attribution: [
          driver({ description: "Every discount dollar comes straight off net sales" })
        ]
      }),
      "why?"
    );
    expect(prompt).toContain(
      "relationship note: Every discount dollar comes straight off net sales"
    );
  });

  it("omits the driver section entirely when there are no drivers", () => {
    const prompt = buildFollowUpPrompt(anomaly(), null, result(), "why?");
    expect(prompt).not.toContain("Declared drivers");
  });
});

describe("warningMessage", () => {
  it("renders each warning kind as one sentence", () => {
    expect(
      warningMessage({
        type: "simpsons_paradox",
        dimension: "location",
        aggregate_delta: -589.39,
        segment_directions: [
          ["Downtown", 41.2],
          ["Uptown", 17.9]
        ]
      })
    ).toContain("aggregate moved -589.39");
    expect(
      warningMessage({
        type: "opposing_offset",
        component_a: "sales.gross",
        delta_a: 120.5,
        component_b: "sales.refunds",
        delta_b: -118.2
      })
    ).toContain("sales.gross +120.50 cancels with sales.refunds -118.20");
  });
});
