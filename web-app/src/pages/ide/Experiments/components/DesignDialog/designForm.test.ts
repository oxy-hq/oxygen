import { describe, expect, it } from "vitest";
import { definitionFixture } from "../../fixtures";
import { emptyDesignValues, toFormValues } from "./designForm";
import { designResolver } from "./resolver";
import { placeServerError } from "./serverErrors";
import { nameProblem, toConfig } from "./toConfig";

describe("toConfig", () => {
  it("sends a value that differs from the server's default, and only then", () => {
    const values = toFormValues({
      ...definitionFixture,
      lever: { ...definitionFixture.lever, on_close: "keep", per: "wave" },
      design: { ...definitionFixture.design, coverage_floor: 0.95, compliance_floor: 0.8 },
      outcome: { ...definitionFixture.outcome, alpha: 0.2, power: 0.9, family: 2 }
    });
    const { config, errors } = toConfig(values);

    expect(errors).toEqual({});
    expect(config.lever).toMatchObject({ on_close: "keep", per: "wave" });
    expect(config.design?.coverage_floor).toBe(0.95);
    expect(config.design).not.toHaveProperty("compliance_floor");
    expect(config.outcome).toMatchObject({ alpha: 0.2, power: 0.9, family: 2 });
  });

  it("round-trips a priced switchback, with its periods and frozen prior", () => {
    const values = toFormValues({
      ...definitionFixture,
      design: {
        ...definitionFixture.design,
        waves: [],
        pre_days: null,
        post_days: null,
        anticipation_days: 0,
        switchback: {
          period_days: 7,
          periods: [
            { from: "2026-03-02", state: "on" },
            { from: "2026-03-09", state: "off" }
          ]
        }
      },
      outcome: {
        ...definitionFixture.outcome,
        prior_coefficient: { value: 0.4, source: "fitted" }
      }
    });
    expect(values.kind).toBe("switchback");
    const { config } = toConfig(values);

    expect(config.design).toMatchObject({
      switchback: {
        period_days: 7,
        periods: [
          { from: "2026-03-02", state: "on" },
          { from: "2026-03-09", state: "off" }
        ]
      }
    });
    expect(config.design).not.toHaveProperty("waves");
    expect(config.design).not.toHaveProperty("pre_days");
    expect(config.outcome?.prior_coefficient).toEqual({ value: 0.4, source: "fitted" });
  });

  it("builds a condition guardrail, and refuses an anomaly one without its measure", () => {
    const values = {
      ...emptyDesignValues(),
      guardrails: [
        { kind: "trigger" as const, measure: "", significance: "", when: "{{o.x}} > 1" },
        { kind: "anomaly" as const, measure: "", significance: "2", when: "" }
      ]
    };
    const { config, errors } = toConfig(values);

    expect(config.guardrails?.[0]).toEqual({
      criteria: { type: "trigger", when: "{{o.x}} > 1" },
      action: "abort"
    });
    expect(errors["guardrails.1.measure"]).toBe("Enter the measure to watch.");
    expect(errors["guardrails.1.significance"]).toBe("Significance must be above 0 and below 1.");
  });

  it("keeps a pricing seed of up to 53 bits exactly", () => {
    const values = toFormValues({
      ...definitionFixture,
      design: { ...definitionFixture.design, seed: 2 ** 53 - 1 }
    });
    expect(toConfig(values).config.design?.seed).toBe(2 ** 53 - 1);
  });

  it("splits unit names on commas and new lines and refuses a bad date", () => {
    const values = {
      ...emptyDesignValues(),
      waves: [{ switchOn: "2026-02-31", units: "a, b\nc,\n" }]
    };
    const { config, errors } = toConfig(values);

    expect(config.design?.waves?.[0].units).toEqual(["a", "b", "c"]);
    expect(errors["waves.0.switchOn"]).toBe("Enter a calendar date.");
  });
});

describe("the grain", () => {
  it("is left out at day grain and sent otherwise, and round-trips", () => {
    const day = toConfig(toFormValues(definitionFixture));
    expect(day.config.design).not.toHaveProperty("grain");
    const values = toFormValues({
      ...definitionFixture,
      design: { ...definitionFixture.design, grain: "week" }
    });
    expect(values.grain).toBe("week");
    const { config, errors } = toConfig(values);
    expect(errors).toEqual({});
    expect(config.design?.grain).toBe("week");
  });

  it("refuses a switch date that does not start a week, on that wave's date", () => {
    const values = toFormValues({
      ...definitionFixture,
      design: {
        ...definitionFixture.design,
        grain: "week",
        waves: [{ switch_on: "2026-03-18", units: ["alice"] }]
      }
    });
    expect(toConfig(values).errors["waves.0.switchOn"]).toMatch(/Monday/);
  });
});

describe("the experiment name", () => {
  const named = (experiment: string) => ({ ...emptyDesignValues(), experiment });

  it("is letters, digits and underscores, once a new experiment is checked", () => {
    expect(nameProblem("customer_orders_2")).toBeUndefined();
    expect(nameProblem("my pilot")).toMatch(/letters, digits and underscores/);
    expect(nameProblem("a-b")).toMatch(/letters, digits and underscores/);
    expect(toConfig(named("my pilot"), { checkName: true }).errors.experiment).toMatch(
      /letters, digits and underscores/
    );
  });

  it("is not judged unless asked, so the saved config is the same as before", () => {
    const { config, errors } = toConfig(named("my pilot"));
    expect(errors.experiment).toBeUndefined();
    expect(config.experiment).toBe("my pilot");
  });

  it("is checked by the form for a new experiment, and left alone for an existing one", async () => {
    const options = {
      criteriaMode: "firstError" as const,
      shouldUseNativeValidation: false,
      fields: {}
    };
    const fresh = await designResolver(named("my pilot"), { nameLocked: false }, options);
    expect(JSON.stringify(fresh.errors)).toMatch(/letters, digits and underscores/);
    const locked = await designResolver(named("my pilot"), { nameLocked: true }, options);
    expect(JSON.stringify(locked.errors)).not.toMatch(/letters, digits and underscores/);
  });
});

describe("placeServerError", () => {
  it("places a guardrail's reason on its significance, numbered from 1", () => {
    const placed = placeServerError(
      "the design is not valid: guardrails[1] anomaly significance is 2, which is not in (0, 1)"
    );
    expect(placed.fields).toEqual({
      "guardrails.1.significance": "Guardrail 2 anomaly significance is 2, which is not in (0, 1)"
    });
  });

  it("does not mistake the English word for a one-word key", () => {
    const placed = placeServerError(
      "the design is not valid: the owner of this experiment could not be notified"
    );
    expect(placed.fields).toEqual({});
    expect(placed.general).toEqual(["The owner of this experiment could not be notified"]);
  });

  it("reads a deserialization path the JSON extractor reports", () => {
    const placed = placeServerError(
      "Failed to deserialize the JSON body into the target type: config.design.timezone: unknown time zone at line 1 column 9"
    );
    expect(placed.fields.timezone).toContain('"Time zone": unknown time zone');
  });

  it("puts a wave's reason on the waves, with its date in words", () => {
    const placed = placeServerError(
      "the design is not valid: wave 2 (switch_on 2026-03-23) lists no units"
    );
    expect(placed.fields.waves).toBe("Wave 2 (switch date 2026-03-23) lists no units");
  });
});

describe("an observed design", () => {
  const observed = () => {
    const { lever: _lever, ...noLever } = definitionFixture;
    return {
      ...noLever,
      design: { ...definitionFixture.design, assignment: "observed" as const, seed: null }
    };
  };

  it("round-trips without a lever and keeps its declared assignment", () => {
    const values = toFormValues(observed());
    expect(values.assignment).toBe("observed");
    const { config, errors } = toConfig(values);
    expect(errors).toEqual({});
    expect(config).not.toHaveProperty("lever");
    expect(config.design?.assignment).toBe("observed");
    expect(config.design?.waves).toEqual(definitionFixture.design.waves);
  });

  it("does not ask for the lever's automations", () => {
    const values = { ...toFormValues(observed()), onVia: "", offVia: "", whenOn: "", whenOff: "" };
    expect(toConfig(values).errors).toEqual({});
  });

  it("a randomised design still needs its lever", () => {
    const values = { ...toFormValues(definitionFixture), onVia: "" };
    expect(values.assignment).toBe("randomised");
    expect(toConfig(values).errors).toHaveProperty("onVia");
  });
});

describe("a ratio outcome", () => {
  it("round-trips its denominator, and omits it when empty", () => {
    const withRatio = {
      ...definitionFixture,
      outcome: { ...definitionFixture.outcome, denominator: "store_days.covers" }
    };
    const values = toFormValues(withRatio);
    expect(values.denominator).toBe("store_days.covers");
    expect(toConfig(values).config.outcome?.denominator).toBe("store_days.covers");
    const plain = toConfig(toFormValues(definitionFixture)).config;
    expect(plain.outcome).not.toHaveProperty("denominator");
  });
});
