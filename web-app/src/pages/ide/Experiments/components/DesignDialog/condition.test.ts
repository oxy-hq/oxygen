import { describe, expect, it } from "vitest";
import { composeCondition, parseCondition } from "./condition";

describe("parseCondition", () => {
  it("reads the one shape the server accepts", () => {
    expect(parseCondition("{{orders.total_order_value}} > 0")).toEqual({
      measure: "orders.total_order_value",
      op: ">",
      value: "0"
    });
    expect(parseCondition("  {{ orders.refund_rate }}>=0.1 ")).toEqual({
      measure: "orders.refund_rate",
      op: ">=",
      value: "0.1"
    });
    expect(parseCondition("{{a.b}} != -2e3")?.op).toBe("!=");
  });

  it("reads two-character operators before one-character ones", () => {
    expect(parseCondition("{{a.b}} <= 5")?.op).toBe("<=");
    expect(parseCondition("{{a.b}} == 5")?.op).toBe("==");
  });

  it.each([
    "",
    "orders.total > 0",
    "{{orders}} > 0",
    "{{a.b.c}} > 0",
    "{{a.b}} > ",
    "{{a.b}} > many",
    "{{a.b}} > 1 and {{a.c}} > 2",
    "{{a.b}} = 3",
    "{{a.b}} > inf"
  ])("does not read %j", (text) => {
    expect(parseCondition(text)).toBeNull();
  });
});

describe("composeCondition", () => {
  it("writes the string the form has always stored", () => {
    expect(composeCondition({ measure: "orders.total_order_value", op: ">=", value: "0" })).toBe(
      "{{orders.total_order_value}} >= 0"
    );
  });

  it("round-trips what it writes", () => {
    const c = { measure: "orders.refund_rate", op: "<" as const, value: "0.25" };
    expect(parseCondition(composeCondition(c))).toEqual(c);
  });
});

describe("ratio conditions (trigger guardrails only)", () => {
  it("reads and writes a ratio of two measures when a ratio is allowed", () => {
    const text = "{{ads.spend}} / {{ads.sales}} > 0.6";
    expect(parseCondition(text, { ratio: true })).toEqual({
      measure: "ads.spend",
      denominator: "ads.sales",
      op: ">",
      value: "0.6"
    });
    expect(
      composeCondition({ measure: "ads.spend", denominator: "ads.sales", op: ">", value: "0.6" })
    ).toBe(text);
  });

  it("does not read a ratio where only one measure is allowed (read-back)", () => {
    expect(parseCondition("{{ads.spend}} / {{ads.sales}} > 0.6")).toBeNull();
  });

  it("writes one measure when the denominator is left empty", () => {
    expect(composeCondition({ measure: "a.b", denominator: "", op: ">=", value: "1" })).toBe(
      "{{a.b}} >= 1"
    );
  });
});
