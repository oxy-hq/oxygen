import { describe, expect, it } from "vitest";
import type { DriverAttribution } from "@/types/metricTree";
import {
  driverPush,
  driverRole,
  groupDrivers,
  roleBadge,
  roleInstruction
} from "./driverClassification";

function driver(overrides: Partial<DriverAttribution> = {}): DriverAttribution {
  return {
    driver_measure: "sales.total_discounts",
    driver_previous: 1_000,
    driver_current: 939.42,
    driver_delta: -60.58,
    direction: "negative",
    contribution: "counteracting",
    form: "linear",
    ...overrides
  };
}

describe("driverRole", () => {
  it("reads an explicit classification", () => {
    expect(driverRole(driver({ contribution: "contributing" }))).toBe("contributing");
    expect(driverRole(driver({ contribution: "counteracting" }))).toBe("counteracting");
    expect(driverRole(driver({ contribution: "unknown" }))).toBe("unresolved");
  });

  // `explain_cache` rows are served back verbatim with no schema version, so an
  // explain cached before classification shipped has no `contribution` at all.
  it("treats an absent contribution as unresolved, not as a default", () => {
    expect(driverRole(driver({ contribution: undefined }))).toBe("unresolved");
  });

  // Mechanical outranks the sign split even when the backend also classified it:
  // "it moved because its base moved" is the dominant fact.
  it("puts a passthrough driver ahead of its own sign", () => {
    const mechanical = driver({
      contribution: "counteracting",
      passthrough: {
        base_measure: "sales.total_gross_sales",
        ratio_previous: 0.0964,
        ratio_current: 0.1002,
        base_driven_delta: -62.68,
        ratio_driven_delta: 2.11
      }
    });
    expect(driverRole(mechanical)).toBe("mechanical");
  });

  // The regression this classification exists to prevent: a value outside this
  // build's union must land somewhere, not fall out of every group.
  it("routes an unrecognized contribution to unresolved", () => {
    const future = { ...driver(), contribution: "ambiguous" } as unknown as DriverAttribution;
    expect(driverRole(future)).toBe("unresolved");
  });
});

describe("groupDrivers", () => {
  it("partitions by role and keeps airlayer's order within a group", () => {
    const a = driver({ driver_measure: "a", contribution: "contributing" });
    const b = driver({ driver_measure: "b", contribution: "contributing" });
    const c = driver({ driver_measure: "c", contribution: "counteracting" });
    const grouped = groupDrivers([b, c, a]);
    expect(grouped.contributing.map((d) => d.driver_measure)).toEqual(["b", "a"]);
    expect(grouped.counteracting.map((d) => d.driver_measure)).toEqual(["c"]);
    expect(grouped.mechanical).toEqual([]);
    expect(grouped.unresolved).toEqual([]);
  });

  it("loses no driver, whatever the contribution holds", () => {
    const drivers = [
      driver({ driver_measure: "a", contribution: "contributing" }),
      driver({ driver_measure: "b", contribution: "counteracting" }),
      driver({ driver_measure: "c", contribution: "unknown" }),
      driver({ driver_measure: "d", contribution: undefined }),
      // Deliberately outside the union, as above: a newer backend's value must
      // still be rendered by this build, and the type forbids writing it.
      {
        ...driver({ driver_measure: "e" }),
        contribution: "ambiguous"
      } as unknown as DriverAttribution,
      driver({
        driver_measure: "f",
        passthrough: {
          base_measure: "sales.total_gross_sales",
          ratio_previous: 0.0964,
          ratio_current: 0.1002,
          base_driven_delta: -62.68,
          ratio_driven_delta: 2.11
        }
      })
    ];
    const grouped = groupDrivers(drivers);
    const rendered = [
      ...grouped.contributing,
      ...grouped.counteracting,
      ...grouped.mechanical,
      ...grouped.unresolved
    ];
    expect(rendered).toHaveLength(drivers.length);
    expect(new Set(rendered.map((d) => d.driver_measure))).toEqual(
      new Set(["a", "b", "c", "d", "e", "f"])
    );
  });

  it("flags staleness only when a contribution is absent", () => {
    expect(groupDrivers([driver({ contribution: undefined })]).anyStale).toBe(true);
    // An explicit JSON null means the same thing as an absent key: never written.
    const nulled = { ...driver(), contribution: null } as unknown as DriverAttribution;
    expect(groupDrivers([nulled]).anyStale).toBe(true);
    // "unknown" is the classifier declining, not a stale row — the panel should
    // advise declaring a direction, not hitting Refresh.
    expect(groupDrivers([driver({ contribution: "unknown" })]).anyStale).toBe(false);
    expect(groupDrivers([driver({ contribution: "contributing" })]).anyStale).toBe(false);
  });

  it("handles an empty driver list", () => {
    const grouped = groupDrivers([]);
    expect(grouped).toEqual({
      contributing: [],
      counteracting: [],
      mechanical: [],
      unresolved: [],
      anyStale: false
    });
  });
});

describe("driverPush", () => {
  const targetDelta = -589.39;

  it("points a contributing driver the way the target moved", () => {
    expect(driverPush(driver({ contribution: "contributing" }), targetDelta)).toBe(-589.39);
  });

  // The original misread: discounts *fell* (Δ -60.58) during a drop, but against
  // a negative relationship that pushes net sales up.
  it("points a counteracting driver against the target's move", () => {
    expect(driverPush(driver({ contribution: "counteracting" }), targetDelta)).toBe(589.39);
  });

  it("makes no claim for unresolved or mechanical drivers", () => {
    expect(driverPush(driver({ contribution: undefined }), targetDelta)).toBeNull();
    expect(driverPush(driver({ contribution: "unknown" }), targetDelta)).toBeNull();
    expect(
      driverPush(
        driver({
          contribution: "contributing",
          passthrough: {
            base_measure: "sales.total_gross_sales",
            ratio_previous: 0.0964,
            ratio_current: 0.1002,
            base_driven_delta: -62.68,
            ratio_driven_delta: 2.11
          }
        }),
        targetDelta
      )
    ).toBeNull();
  });
});

describe("labels", () => {
  it("names the tracked base on a mechanical driver, unprefixed", () => {
    const mechanical = driver({
      passthrough: {
        base_measure: "sales.total_gross_sales",
        ratio_previous: 0.0964,
        ratio_current: 0.1002,
        base_driven_delta: -62.68,
        ratio_driven_delta: 2.11
      }
    });
    expect(roleBadge(mechanical)).toBe("tracks total_gross_sales");
    expect(roleInstruction(mechanical)).toContain("do not cite as a cause or an offset");
  });

  it("never calls a counteracting driver a cause", () => {
    const instruction = roleInstruction(driver({ contribution: "counteracting" }));
    expect(roleBadge(driver({ contribution: "counteracting" }))).toBe("offsets");
    expect(instruction).toContain("did NOT cause it");
  });

  it("says the direction is undetermined rather than guessing", () => {
    for (const contribution of ["unknown", undefined] as const) {
      expect(roleBadge(driver({ contribution }))).toBe("direction undetermined");
      expect(roleInstruction(driver({ contribution }))).toContain("cannot say which way it pushed");
    }
  });

  it("labels a contributing driver as explaining the move", () => {
    expect(roleBadge(driver({ contribution: "contributing" }))).toBe("explains");
    expect(roleInstruction(driver({ contribution: "contributing" }))).toBe(
      "explains part of the move"
    );
  });
});
