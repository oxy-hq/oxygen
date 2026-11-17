import { describe, expect, it } from "vitest";
import type { CostDay } from "@/services/api/adminObservability";
import { alignWindow } from "./dailySeries";

function day(d: string, cost: number): CostDay {
  return { day: d, cost_usd: cost, input_tokens: 0, output_tokens: 0, run_count: 1 };
}

const NOW = new Date("2026-11-17T10:00:00Z");

describe("alignWindow", () => {
  it("fills every calendar day of the window, oldest first", () => {
    const s = alignWindow([day("2026-11-17", 5)], [], 7, NOW);
    expect(s).toHaveLength(8);
    expect(s[0].day).toBe("2026-11-10");
    expect(s[7]).toMatchObject({ day: "2026-11-17", cost_usd: 5 });
    // A quiet day is a measured zero, not a missing bar.
    expect(s[3].cost_usd).toBe(0);
  });

  it("pairs each day with the same day one window earlier", () => {
    const s = alignWindow([day("2026-11-17", 5)], [day("2026-11-10", 2)], 7, NOW);
    expect(s[7].prior_cost_usd).toBe(2);
    expect(s[0].prior_cost_usd).toBe(0);
  });

  it("draws no prior when the prior window was not read", () => {
    const s = alignWindow([day("2026-11-17", 5)], null, 7, NOW);
    expect(s.every((d) => d.prior_cost_usd === null)).toBe(true);
  });
});
