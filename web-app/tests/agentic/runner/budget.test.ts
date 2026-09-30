import { describe, expect, it } from "vitest";
import {
  BudgetExceeded,
  charge,
  createMeter,
  ensurePriced,
  remaining,
  reserve,
  worstCaseUsd
} from "./budget";

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

  it("is a no-op when nothing is metered — the test suite runs unchanged", () => {
    expect(() => charge(undefined, 100)).not.toThrow();
    expect(() => reserve(undefined, 100)).not.toThrow();
    expect(() => ensurePriced(undefined, "some-unpriced-model")).not.toThrow();
  });

  it("rejects a budget that is not positive", () => {
    expect(() => createMeter(0)).toThrow();
    expect(() => createMeter(Number.NaN)).toThrow();
  });
});
