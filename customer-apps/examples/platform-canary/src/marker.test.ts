// The I5 marker the release journey's browser check waits for. A marker that
// reads `ready` before every check passed would let that check pass without
// doing its work, so each state is pinned here.

import { describe, expect, it } from "vitest";
import type { CheckName } from "./checks";
import { CHECK_ORDER, canaryMarker, type Outcome } from "./marker";

type Outcomes = Partial<Record<CheckName, Outcome>>;

describe("canaryMarker", () => {
  it("runs the checks in the documented order", () => {
    expect(CHECK_ORDER).toEqual(["echo", "sql_query_route", "sdk_query"]);
  });

  it("marks ready only when all three checks passed", () => {
    const outcomes: Outcomes = { echo: "pass", sql_query_route: "pass", sdk_query: "pass" };
    expect(canaryMarker({ manifest: "loaded", outcomes })).toEqual({
      "data-canary": "ready",
      "aria-busy": false
    });
  });

  it.each([
    ["nothing has run", {}],
    ["echo passed", { echo: "pass" }],
    ["echo and sql_query_route passed", { echo: "pass", sql_query_route: "pass" }],
    ["only the last check passed", { sdk_query: "pass" }]
  ] as Array<[string, Outcomes]>)(
    "renders no marker while a check is pending: %s",
    (_label, outcomes) => {
      expect(canaryMarker({ manifest: "loaded", outcomes })).toEqual({ "aria-busy": true });
    }
  );

  it.each(CHECK_ORDER)("marks %s when it is the check that failed", (failed) => {
    const outcomes: Outcomes = {};
    for (const check of CHECK_ORDER) {
      if (check === failed) break;
      outcomes[check] = "pass";
    }
    outcomes[failed] = "fail";
    expect(canaryMarker({ manifest: "loaded", outcomes })).toEqual({
      "data-canary-failed": failed,
      "aria-busy": false
    });
  });

  it("names the first failure in check order, not the last", () => {
    const outcomes: Outcomes = { sdk_query: "fail", sql_query_route: "fail", echo: "pass" };
    expect(canaryMarker({ manifest: "loaded", outcomes })).toEqual({
      "data-canary-failed": "sql_query_route",
      "aria-busy": false
    });
  });

  it("never marks ready once a check failed, even with the others passed", () => {
    const outcomes: Outcomes = { echo: "pass", sql_query_route: "pass", sdk_query: "fail" };
    const marker = canaryMarker({ manifest: "loaded", outcomes });
    expect(marker["data-canary"]).toBeUndefined();
    expect(marker["data-canary-failed"]).toBe("sdk_query");
  });

  it("marks manifest when the manifest failed to load", () => {
    expect(canaryMarker({ manifest: "failed" })).toEqual({ "data-canary-failed": "manifest" });
  });
});
