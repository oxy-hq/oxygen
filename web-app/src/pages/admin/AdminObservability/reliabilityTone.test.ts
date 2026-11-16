import { describe, expect, it } from "vitest";
import { isReliabilityConcern, reliabilityTone } from "./reliabilityTone";

describe("reliabilityTone", () => {
  /**
   * The rule this file exists for. An org with no traffic has **no opinion**
   * about reliability — "nobody opened it" is not "it worked perfectly" — and
   * an unmeasured deployment has even less of one. Drawing either green is a
   * claim nothing supports, and it is the claim an operator would act on.
   */
  it("has no opinion about an org with no traffic, or no store", () => {
    expect(reliabilityTone(null)).toBe("muted");
    expect(reliabilityTone(undefined)).toBe("muted");
    expect(isReliabilityConcern(null)).toBe(false);
  });

  /**
   * A healthy rate takes no colour either. On the staff console colour means
   * something is wrong, so colouring a good number spends the one signal the
   * surface has on reassurance nobody asked for — and makes a real warning
   * harder to spot in a long list.
   */
  it("leaves a healthy rate uncoloured", () => {
    expect(reliabilityTone(0)).toBe("muted");
    expect(reliabilityTone(0.009)).toBe("muted");
    expect(isReliabilityConcern(0.009)).toBe(false);
  });

  /** 1% is the allowance the burn-rate evaluator already works against, so it
   *  is where a rate stops being noise. */
  it("warns from one percent", () => {
    expect(reliabilityTone(0.01)).toBe("warn");
    expect(reliabilityTone(0.049)).toBe("warn");
    expect(isReliabilityConcern(0.01)).toBe(true);
  });

  /** Five times the allowance is where it stops being survivable. */
  it("escalates at five percent", () => {
    expect(reliabilityTone(0.05)).toBe("danger");
    expect(reliabilityTone(1)).toBe("danger");
  });

  /** A NaN from some future division must not render as a danger colour, nor
   *  crash the comparison into a silent pass. */
  it("treats a non-finite rate as no opinion", () => {
    expect(reliabilityTone(Number.NaN)).toBe("muted");
    expect(reliabilityTone(Number.POSITIVE_INFINITY)).toBe("muted");
  });
});
