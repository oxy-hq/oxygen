import { describe, expect, it } from "vitest";
import {
  closeText,
  directionText,
  filterText,
  guardrailText,
  humanize,
  measureLabel,
  percent,
  perText,
  randomisesText,
  significanceText,
  windowsText
} from "./describe";

describe("describe", () => {
  it("humanizes snake_case", () => {
    expect(humanize("vendor_note")).toBe("Vendor note");
    expect(humanize("x")).toBe("X");
  });

  it("labels a measure from its semantic ref", () => {
    expect(measureLabel("shopper_days.order_value")).toBe("Order value per shopper-day");
    expect(measureLabel("orders.total_orders")).toBe("Total orders");
    expect(measureLabel("plain")).toBe("Plain");
  });

  it("words who is randomised", () => {
    expect(randomisesText("shopper", null)).toBe("Randomises each shopper");
    expect(randomisesText("sku", "brand")).toBe("Randomises groups of sku by brand");
  });

  it("words direction, significance, percent", () => {
    expect(directionText("increase")).toBe("expects an increase");
    expect(directionText("decrease")).toBe("expects a decrease");
    expect(significanceText(0.05)).toBe("α 0.05");
    expect(significanceText(0.2)).toBe("Directional, α 0.2");
    expect(percent(0.9)).toBe("90%");
    expect(percent(0.875)).toBe("87.5%");
  });

  it("words the lever", () => {
    expect(closeText("off")).toBe("Turned off when the experiment closes");
    expect(closeText("keep")).toBe("Left on when the experiment closes");
    expect(perText("unit")).toMatch(/each unit/);
    expect(perText("wave")).toMatch(/each wave/);
  });

  it("words the windows", () => {
    expect(
      windowsText({ pre_days: 28, post_days: 28, anticipation_days: 0, washout_days: 3 })
    ).toBe("28 days before and 28 days after each switch; no anticipation buffer; 3-day washout.");
    expect(
      windowsText({ pre_days: null, post_days: null, anticipation_days: 2, washout_days: 0 })
    ).toBe("2-day anticipation buffer; no washout.");
  });

  it("words guardrails", () => {
    expect(
      guardrailText({
        criteria: { type: "anomaly", measure: "orders.refund_rate", significance: 0.01 },
        action: "abort"
      })
    ).toBe("Aborts the run if Refund rate looks anomalous (significance 0.01).");
    expect(
      guardrailText({ criteria: { type: "trigger", when: "{{a.b}} > 1" }, action: "abort" })
    ).toBe("Aborts the run when:");
  });
});

describe("filterText", () => {
  it("reads a measure's filters as plain words, without the view", () => {
    expect(filterText(undefined)).toBe("");
    expect(
      filterText([
        { dimension: "market_sku_days.category", op: "equals", values: ["backpacks"] },
        { dimension: "market_sku_days.sku", op: "not_equals", values: ["TB-40", "TB-30"] }
      ])
    ).toBe("only rows where category is backpacks and sku is not TB-40 or TB-30");
  });
});
