import { describe, expect, it } from "vitest";
import { definitionFixture } from "../../fixtures";
import { toFormValues } from "./designForm";
import { toConfig } from "./toConfig";

const filtered = {
  ...definitionFixture,
  outcome: {
    ...definitionFixture.outcome,
    target: "market_sku_days.revenue",
    target_filter: [{ dimension: "market_sku_days.sku", op: "equals" as const, values: ["TB-40"] }]
  },
  substitution_check: {
    sibling: "market_sku_days.revenue",
    filter: [
      { dimension: "market_sku_days.category", op: "equals" as const, values: ["backpacks"] },
      { dimension: "market_sku_days.sku", op: "not_equals" as const, values: ["TB-40"] }
    ]
  }
};

describe("measure filters", () => {
  it("round-trip through the form for the outcome and the sibling", () => {
    const values = toFormValues(filtered);
    expect(values.targetFilter).toEqual([
      { dimension: "market_sku_days.sku", op: "equals", values: ["TB-40"] }
    ]);
    expect(values.siblingFilter).toHaveLength(2);

    const { config, errors } = toConfig(values);
    expect(errors).toEqual({});
    expect(config.outcome?.target_filter).toEqual(filtered.outcome.target_filter);
    expect(config.outcome).not.toHaveProperty("driver_filter");
    expect(config.substitution_check?.filter).toEqual(filtered.substitution_check.filter);
  });

  it("sends no filter key for a design without one", () => {
    const { config } = toConfig(toFormValues(definitionFixture));
    expect(config.outcome).not.toHaveProperty("target_filter");
    expect(config.substitution_check).not.toHaveProperty("filter");
  });

  it("asks for a dimension and a value on an incomplete row", () => {
    const values = toFormValues(filtered);
    values.targetFilter = [{ dimension: "", op: "equals", values: [] }];
    const { errors } = toConfig(values);
    expect(errors["targetFilter.0.dimension"]).toBe("Choose what to narrow the measure by.");
    expect(errors["targetFilter.0.values"]).toBe("Choose at least one value.");
  });
});
