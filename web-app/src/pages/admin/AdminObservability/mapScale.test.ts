import { describe, expect, it } from "vitest";
import {
  axisCount,
  axisUsd,
  decadeTicks,
  logDomain,
  logPosition,
  perRunGuides,
  perRunLabel
} from "./mapScale";

describe("logDomain", () => {
  it("covers every positive value with whole decades", () => {
    expect(logDomain([3, 450, 1_842])).toEqual([1, 10_000]);
  });
  it("ignores zeros, which a log axis cannot place", () => {
    expect(logDomain([0, 0.4, 7])).toEqual([0.1, 10]);
  });
  it("is null when nothing is positive", () => {
    expect(logDomain([0, 0])).toBeNull();
  });
  it("never has zero width", () => {
    expect(logDomain([100])).toEqual([100, 1_000]);
  });
});

describe("logPosition", () => {
  it("places decades evenly", () => {
    expect(logPosition(1, [1, 1_000])).toBe(0);
    expect(logPosition(10, [1, 1_000])).toBeCloseTo(1 / 3);
    expect(logPosition(1_000, [1, 1_000])).toBe(1);
  });
});

describe("decadeTicks", () => {
  it("ticks every decade inclusive", () => {
    expect(decadeTicks([0.1, 100])).toEqual([0.1, 1, 10, 100]);
  });
});

describe("perRunGuides", () => {
  it("centres on the median cost per run", () => {
    // Median 0.4 → nearest step 0.5, guides two steps either side.
    expect(perRunGuides([0.2, 0.4, 0.9])).toEqual([0.1, 0.5, 2]);
  });
  it("drops guides that would fall off the step list", () => {
    expect(perRunGuides([0.001])).toEqual([0.001, 0.005]);
  });
  it("draws none without a priced run", () => {
    expect(perRunGuides([])).toEqual([]);
  });
});

describe("labels", () => {
  it("formats axes compactly", () => {
    expect(axisUsd(1_000)).toBe("$1k");
    expect(axisUsd(0.1)).toBe("$0.1");
    expect(axisCount(10_000)).toBe("10k");
  });
  it("formats guides as money per run", () => {
    expect(perRunLabel(0.5)).toBe("$0.50 / run");
    expect(perRunLabel(2)).toBe("$2 / run");
    expect(perRunLabel(0.005)).toBe("$0.005 / run");
  });
});
