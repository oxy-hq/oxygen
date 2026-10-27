import { describe, expect, it } from "vitest";
import { definitionFixture } from "../../fixtures";
import { toFormValues } from "./designForm";
import { toConfig } from "./toConfig";

describe("carryover window", () => {
  it("round-trips the days read after the lever turns off", () => {
    const values = toFormValues({
      ...definitionFixture,
      design: { ...definitionFixture.design, carryover_days: 14 }
    });
    expect(values.carryoverDays).toBe("14");
    const { config, errors } = toConfig(values);
    expect(errors).toEqual({});
    expect(config.design?.carryover_days).toBe(14);
  });

  it("sends no key when left empty", () => {
    const { config } = toConfig(toFormValues(definitionFixture));
    expect(config.design).not.toHaveProperty("carryover_days");
  });

  it("asks for at least one day", () => {
    const values = toFormValues(definitionFixture);
    values.carryoverDays = "0";
    expect(toConfig(values).errors.carryoverDays).toBe("Enter a whole number of 1 or more.");
  });
});
