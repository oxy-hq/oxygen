import { describe, expect, it } from "vitest";
import type { FlipCounts, UnitGuardrail } from "@/types/experiment";
import {
  flipTotal,
  groupRows,
  membershipTotals,
  pairOf,
  periodRows,
  periodStatus,
  summariseUnits
} from "./operations";

const flips = (over: Partial<FlipCounts> = {}): FlipCounts => ({
  pending: 0,
  sent: 0,
  confirmed: 0,
  unconfirmed: 0,
  failed: 0,
  cancelled: 0,
  ...over
});

describe("periodStatus", () => {
  it("is upcoming until a flip row exists", () => {
    expect(periodStatus(flips())).toBe("upcoming");
  });

  it("is landed only when every flip owed is confirmed", () => {
    expect(periodStatus(flips({ confirmed: 6 }))).toBe("landed");
    expect(periodStatus(flips({ confirmed: 5, sent: 6 }))).toBe("in_progress");
    expect(periodStatus(flips({ pending: 6 }))).toBe("in_progress");
  });

  it("needs attention when any flip failed or could not be confirmed, even if others landed", () => {
    expect(periodStatus(flips({ confirmed: 5, failed: 1 }))).toBe("needs_attention");
    expect(periodStatus(flips({ confirmed: 5, unconfirmed: 1 }))).toBe("needs_attention");
  });

  it("counts a sent flip once: sent overlaps the settled buckets", () => {
    // Server: `sent` is every flip that went out, settled or not.
    expect(flipTotal(flips({ sent: 6, confirmed: 6 }))).toBe(6);
    expect(periodStatus(flips({ sent: 6, confirmed: 6 }))).toBe("landed");
    expect(flipTotal(flips({ sent: 2, confirmed: 1, pending: 0 }))).toBe(2);
    expect(flipTotal(flips({ sent: 3, confirmed: 1, failed: 1, pending: 2 }))).toBe(5);
  });

  it("does not owe a cancelled flip", () => {
    expect(flipTotal(flips({ confirmed: 3, cancelled: 2 }))).toBe(3);
    expect(periodStatus(flips({ confirmed: 3, cancelled: 2 }))).toBe("landed");
  });
});

describe("periods", () => {
  it("numbers consecutive periods in pairs: 0 and 1 are pair 1", () => {
    expect([0, 1, 2, 3, 4].map(pairOf)).toEqual([1, 1, 2, 2, 3]);
  });

  it("derives a row per period with its progress", () => {
    const rows = periodRows([
      {
        index: 0,
        from: "2026-03-02",
        state: "on",
        flips: flips({ confirmed: 4, failed: 1, sent: 6 })
      },
      { index: 1, from: "2026-03-09", state: "off", flips: flips() }
    ]);
    expect(rows).toEqual([
      {
        index: 0,
        pair: 1,
        from: "2026-03-02",
        state: "on",
        confirmed: 4,
        total: 6,
        failed: 1,
        status: "needs_attention"
      },
      {
        index: 1,
        pair: 1,
        from: "2026-03-09",
        state: "off",
        confirmed: 0,
        total: 0,
        failed: 0,
        status: "upcoming"
      }
    ]);
  });
});

describe("groups", () => {
  it("lists groups by name, and an unpinned membership as none", () => {
    expect(groupRows(null)).toEqual([]);
    expect(
      groupRows({ Snacks: ["d", "e"], Beverages: ["a", "b", "c"] }).map((r) => r.group)
    ).toEqual(["Beverages", "Snacks"]);
  });

  it("totals groups and members", () => {
    expect(membershipTotals(groupRows({ A: ["x", "y"], B: ["z"] }))).toEqual({
      groups: 2,
      members: 3
    });
    expect(membershipTotals([])).toEqual({ groups: 0, members: 0 });
  });
});

describe("summariseUnits", () => {
  const unit = (name: string, status: UnitGuardrail["status"]): UnitGuardrail => ({
    unit: name,
    status,
    detail: null
  });

  it("separates the breached, the unevaluated and the count that are fine", () => {
    const s = summariseUnits([
      unit("bondi", "breached"),
      unit("manly", "ok"),
      unit("coogee", "refused"),
      unit("newtown", "not_checked"),
      unit("surry", "ok")
    ]);
    expect(s.breached.map((u) => u.unit)).toEqual(["bondi"]);
    expect(s.unevaluated.map((u) => u.unit)).toEqual(["coogee", "newtown"]);
    expect(s.ok).toBe(2);
  });
});
