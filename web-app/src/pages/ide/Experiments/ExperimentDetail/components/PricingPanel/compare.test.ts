import { describe, expect, it } from "vitest";
import {
  describeCandidate,
  formatOwnMde,
  GROUP_CAVEAT,
  isGroupDesign,
  rankCandidates
} from "./compare";
import { failed, grouped, pending, proposal, refused, switchback } from "./fixtures";

describe("rankCandidates", () => {
  it("ranks by mde_unit_day, never by the design's own-unit mde", () => {
    const unit = proposal({ id: "unit", mde: 17.5, mde_unit_day: 17.5 });
    // $1,200/group-day is the biggest raw number and the smallest per unit-day.
    const group = grouped("group", { mde: 1200, mde_unit_day: 9 });
    const sw = switchback("sw", { mde: 12, mde_unit_day: 12 });

    const ranked = rankCandidates([unit, group, sw]);

    expect(ranked.map((r) => [r.proposal.id, r.rank])).toEqual([
      ["group", 1],
      ["sw", 2],
      ["unit", 3]
    ]);
  });

  it("keeps request order for equal MDEs", () => {
    const a = proposal({ id: "a", mde_unit_day: 10 });
    const b = proposal({ id: "b", mde_unit_day: 10 });
    expect(rankCandidates([a, b]).map((r) => r.proposal.id)).toEqual(["a", "b"]);
    expect(rankCandidates([b, a]).map((r) => r.proposal.id)).toEqual(["b", "a"]);
  });

  it("leaves refused, failed, pending and uncomparable candidates unranked, after the ranked ones, in request order", () => {
    const ranked = rankCandidates([
      refused("r", "no history"),
      proposal({ id: "slow", mde_unit_day: 30 }),
      failed("f", "faulted"),
      proposal({ id: "no-scale", mde_unit_day: null }),
      pending("p"),
      proposal({ id: "fast", mde_unit_day: 5 })
    ]);

    expect(ranked.map((r) => [r.proposal.id, r.rank])).toEqual([
      ["fast", 1],
      ["slow", 2],
      ["r", null],
      ["f", null],
      ["no-scale", null],
      ["p", null]
    ]);
  });

  it("does not rank a non-finite MDE", () => {
    const ranked = rankCandidates([
      proposal({ id: "inf", mde_unit_day: Number.POSITIVE_INFINITY })
    ]);
    expect(ranked[0].rank).toBeNull();
  });
});

describe("group designs", () => {
  it("is a group design exactly when the proposal assigns by a dimension", () => {
    expect(isGroupDesign(grouped("g"))).toBe(true);
    expect(isGroupDesign(proposal({ id: "u" }))).toBe(false);
    expect(isGroupDesign(switchback("s"))).toBe(false);
  });

  it("labels the own-unit MDE with its own unit", () => {
    expect(formatOwnMde(grouped("g", { mde: 1200 }))).toBe("$1,200.00/group-day");
    expect(formatOwnMde(proposal({ id: "u", mde: 17.5 }))).toBe("$17.50/unit-day");
    expect(formatOwnMde(switchback("s", { mde: 12 }))).toBe("$12.00/unit-day");
    expect(formatOwnMde(grouped("g", { mde: null }))).toBe("—");
  });

  it("states the even-spread assumption behind a group's per-unit-day figure", () => {
    expect(GROUP_CAVEAT).toContain("mean group size");
    expect(GROUP_CAVEAT).toContain("spreads evenly over a group's members");
  });
});

describe("describeCandidate", () => {
  it("describes a plain staggered design", () => {
    expect(
      describeCandidate({
        shape: "waves",
        wave_sizes: [3, 3],
        first_switch_on: "2035-03-05",
        spacing_days: 14
      })
    ).toBe("Waves 3 + 3, first 2035-03-05, every 14 days");
  });

  it("adds blocks and the group dimension when present", () => {
    expect(
      describeCandidate({
        shape: "waves",
        wave_sizes: [2, 2, 2],
        first_switch_on: "2035-03-05",
        spacing_days: 7,
        blocks: 3,
        assign_by: "category"
      })
    ).toBe("Waves 2 + 2 + 2, first 2035-03-05, every 7 days, 3 blocks, by category");
  });

  it("omits the spacing for a single wave", () => {
    expect(
      describeCandidate({
        shape: "waves",
        wave_sizes: [10],
        first_switch_on: "2026-10-19",
        spacing_days: 1
      })
    ).toBe("Waves 10, first 2026-10-19");
  });

  it("uses the singular for one day of spacing or period", () => {
    expect(
      describeCandidate({
        shape: "waves",
        wave_sizes: [2, 2],
        first_switch_on: "2026-10-19",
        spacing_days: 1
      })
    ).toBe("Waves 2 + 2, first 2026-10-19, every 1 day");
    expect(
      describeCandidate({
        shape: "switchback",
        first_period_on: "2035-03-02",
        period_days: 1,
        pairs: 2
      })
    ).toBe("Switchback 2 pairs × 1 day from 2035-03-02");
  });

  it("describes a switchback by its pairs and period", () => {
    expect(
      describeCandidate({
        shape: "switchback",
        first_period_on: "2035-03-02",
        period_days: 7,
        pairs: 4
      })
    ).toBe("Switchback 4 pairs × 7 days from 2035-03-02");
  });

  it("uses the singular for one pair", () => {
    expect(
      describeCandidate({
        shape: "switchback",
        first_period_on: "2035-03-02",
        period_days: 14,
        pairs: 1
      })
    ).toBe("Switchback 1 pair × 14 days from 2035-03-02");
  });
});
