import { describe, expect, it } from "vitest";
import {
  BudgetExceeded,
  caseBudgetUsd,
  charge,
  createMeter,
  DEFAULT_CASE_BUDGET_USD,
  ensurePriced,
  remaining,
  reserve,
  worstCaseUsd
} from "./budget";

// The suite meters every case run. Before it did, a case whose page never
// loaded spent $3.67 and $6.39 on one CI run, because a step that cannot
// succeed is not a failed step — it runs to `max_steps`.
describe("the suite's per-case budget", () => {
  it("applies the default when the variable is unset or blank", () => {
    expect(caseBudgetUsd({})).toBe(DEFAULT_CASE_BUDGET_USD);
    expect(caseBudgetUsd({ AGENTIC_CASE_BUDGET_USD: "  " })).toBe(DEFAULT_CASE_BUDGET_USD);
  });

  it("takes a dollar amount", () => {
    expect(caseBudgetUsd({ AGENTIC_CASE_BUDGET_USD: "0.5" })).toBe(0.5);
    expect(caseBudgetUsd({ AGENTIC_CASE_BUDGET_USD: " 4 " })).toBe(4);
  });

  it("is unmetered only when asked for by name", () => {
    expect(caseBudgetUsd({ AGENTIC_CASE_BUDGET_USD: "0" })).toBeUndefined();
    expect(caseBudgetUsd({ AGENTIC_CASE_BUDGET_USD: "OFF" })).toBeUndefined();
  });

  it("refuses a value it cannot read rather than running without a limit", () => {
    expect(() => caseBudgetUsd({ AGENTIC_CASE_BUDGET_USD: "two" })).toThrow(/dollar amount/);
    expect(() => caseBudgetUsd({ AGENTIC_CASE_BUDGET_USD: "-1" })).toThrow(/dollar amount/);
  });

  it("is a limit a meter accepts, above the dearest flow's cold budget", () => {
    expect(() => createMeter(DEFAULT_CASE_BUDGET_USD)).not.toThrow();
    // metric-tree-scenario, the dearest entry in flows/_budgets.yml, for two cases.
    expect(DEFAULT_CASE_BUDGET_USD).toBeGreaterThan(1.55);
  });
});

describe("the cost meter", () => {
  it("charges under the limit and stops the call that crosses it", () => {
    const meter = createMeter(0.1);
    charge(meter, 0.06);
    expect(remaining(meter)).toBeCloseTo(0.04);
    expect(() => charge(meter, 0.05)).toThrow(BudgetExceeded);
    expect(() => charge(meter, 0)).toThrow(/stopped at the \$0\.10 budget/);
  });

  it("stops BEFORE a call whose worst case does not fit, and says it stopped", () => {
    const meter = createMeter(0.05);
    // 30k bytes ≈ 15k tokens at the cache-write rate + 1024 output on Sonnet ≈ $0.072
    const worst = worstCaseUsd("claude-sonnet-4-6", 30_000, 1024);
    expect(worst).toBeCloseTo(15_000 * 3.75e-6 + 1024 * 15e-6, 6);
    expect(() => reserve(meter, worst)).toThrow(/next call could cost/);
    expect(meter.spentUsd).toBe(0);
    expect(meter.stopped).toMatch(/\$0\.05 budget/);
    expect(() => reserve(createMeter(1), worst)).not.toThrow();
  });

  it("stays stopped: once it stops a call, even a cheap one after it is refused", () => {
    const meter = createMeter(0.05);
    expect(() => reserve(meter, 0.06)).toThrow(BudgetExceeded);
    expect(() => reserve(meter, 0.001)).toThrow(/next call could cost \$0\.060/);
  });

  it("refuses a model it cannot price, since that would charge $0 forever", () => {
    const meter = createMeter(1);
    expect(() => ensurePriced(meter, "claude-sonnet-4-6")).not.toThrow();
    expect(() => ensurePriced(meter, "some-unpriced-model")).toThrow(/no price/);
  });

  it("is a no-op when there is no meter", () => {
    expect(() => charge(undefined, 100)).not.toThrow();
    expect(() => reserve(undefined, 100)).not.toThrow();
    expect(() => ensurePriced(undefined, "some-unpriced-model")).not.toThrow();
  });

  it("rejects a budget that is not positive", () => {
    expect(() => createMeter(0)).toThrow();
    expect(() => createMeter(Number.NaN)).toThrow();
  });
});
