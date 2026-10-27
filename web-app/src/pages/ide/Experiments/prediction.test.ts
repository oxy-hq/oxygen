import { describe, expect, it } from "vitest";
import { predictionVerdict, priorSourceLabel } from "./prediction";

describe("predictionVerdict", () => {
  it("is agrees when the prior is inside the earned set and contradicts when it is outside", () => {
    expect(predictionVerdict({ prior_in_interval: true })).toBe("agrees");
    expect(predictionVerdict({ prior_in_interval: false })).toBe("contradicts");
  });

  it("is unknown, never contradicts, when the engine could not say", () => {
    // `null` is falsy; reading it as `false` would report a disagreement nobody measured.
    expect(predictionVerdict({ prior_in_interval: null })).toBe("unknown");
  });
});

describe("priorSourceLabel", () => {
  it("names where the prior came from, and passes an unknown source through", () => {
    expect(priorSourceLabel("declared")).toBe("declared in the World Model");
    expect(priorSourceLabel("fitted")).toBe("fitted from history");
    expect(priorSourceLabel("elicited")).toBe("elicited");
  });
});
