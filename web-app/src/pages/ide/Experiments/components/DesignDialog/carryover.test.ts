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

  it("round-trips the date a past change ended on an observed design", () => {
    const values = toFormValues({
      ...definitionFixture,
      design: {
        ...definitionFixture.design,
        assignment: "observed",
        carryover_days: 7,
        waves: [{ switch_on: "2026-05-06", switch_off: "2026-05-18", units: ["kit"] }]
      }
    });
    expect(values.waves[0].switchOff).toBe("2026-05-18");
    const { config, errors } = toConfig(values);
    expect(errors).toEqual({});
    expect(config.design?.waves).toEqual([
      { switch_on: "2026-05-06", switch_off: "2026-05-18", units: ["kit"] }
    ]);
  });

  it("sends no end date for a wave that leaves it empty, or on a randomised design", () => {
    const values = toFormValues(definitionFixture);
    values.waves[0].switchOff = "2026-04-01";
    const waves = toConfig(values).config.design?.waves ?? [];
    expect(waves.every((w) => !("switch_off" in w))).toBe(true);
  });

  it("checks the end date is a calendar date", () => {
    const values = toFormValues({
      ...definitionFixture,
      design: { ...definitionFixture.design, assignment: "observed" }
    });
    values.waves[0].switchOff = "2026-02-31";
    expect(toConfig(values).errors["waves.0.switchOff"]).toBe("Enter a calendar date.");
  });
});
