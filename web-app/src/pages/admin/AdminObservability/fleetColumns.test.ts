import { describe, expect, it } from "vitest";
import { fleetColumns } from "./fleetColumns";

describe("fleetColumns", () => {
  /**
   * The rule this file exists for. Three app columns times every org on the
   * platform is a screen of em-dashes in the half of the table the operator
   * came to read — and worse, it cannot be told apart from "every org is
   * quiet".
   */
  it("drops the app columns when nothing can answer them", () => {
    const { shown, hidden } = fleetColumns({ apps_measured: false });
    const ids = shown.map((c) => c.id);
    expect(ids).not.toContain("requests");
    expect(ids).not.toContain("p95");
    expect(ids).not.toContain("errorRate");
    expect(hidden?.labels).toEqual(["Requests", "p95", "Errors"]);
  });

  /**
   * And the other half of the rule: what is dropped is **named once, with the
   * reason**. A column that vanishes with no explanation is its own small
   * dishonesty — the operator cannot tell "this deployment cannot measure
   * that" from "someone removed the column".
   */
  it("names what it dropped, and why, in terms an operator can act on", () => {
    const { hidden } = fleetColumns({ apps_measured: false });
    expect(hidden).not.toBeNull();
    expect(hidden?.why).toContain("OXY_OBSERVABILITY_BACKEND");
  });

  /**
   * With a store configured the columns are shown even though some orgs will
   * sit at zero — because *those* zeros are information. This is the exception
   * the custom-apps fleet table wrote: gate on the deployment capability, not
   * on the values, because the values cannot tell the two cases apart.
   */
  it("keeps the app columns when the deployment can measure, even if orgs are quiet", () => {
    const { shown, hidden } = fleetColumns({ apps_measured: true });
    expect(shown.map((c) => c.id)).toContain("requests");
    expect(hidden).toBeNull();
  });

  /**
   * Cost is never derived away. It reads Postgres, which every deployment has,
   * so "no spend" is always a measurement — and a list with no spend column
   * would not be this list.
   */
  it("always shows identity and spend", () => {
    for (const apps_measured of [true, false]) {
      const ids = fleetColumns({ apps_measured }).shown.map((c) => c.id);
      expect(ids).toContain("org");
      expect(ids).toContain("spend");
    }
  });

  /** Figures are right-aligned so they read down a column; the org name is not. */
  it("marks every figure column numeric and the name column not", () => {
    const { shown } = fleetColumns({ apps_measured: true });
    expect(shown.find((c) => c.id === "org")?.numeric).toBeFalsy();
    for (const c of shown.filter((c) => c.id !== "org")) {
      expect(c.numeric).toBe(true);
    }
  });
});
