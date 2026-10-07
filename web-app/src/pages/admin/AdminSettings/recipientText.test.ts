import { describe, expect, it } from "vitest";
import { formatDay, reachLabel, roleLabel, turnedOffBy } from "./recipientText";
import { recipient } from "./testFixtures";

describe("roleLabel", () => {
  it("names the two roles that get the report", () => {
    expect(roleLabel("global_owner")).toBe("Global owner");
    expect(roleLabel("global_admin")).toBe("Global admin");
  });

  it("shows a role it has not heard of as the server wrote it", () => {
    // Not prettified: "App operator" would be a guess, and a guess can name the wrong role.
    expect(roleLabel("app_operator")).toBe("app_operator");
    expect(roleLabel("Billing lead")).toBe("Billing lead");
  });
});

describe("reachLabel", () => {
  it("says all organizations for an unscoped grant, whatever the count says", () => {
    expect(reachLabel({ scope_all: true, org_count: null })).toBe("All organizations");
    expect(reachLabel({ scope_all: true, org_count: 3 })).toBe("All organizations");
  });

  it("counts the organizations a scoped grant names", () => {
    expect(reachLabel({ scope_all: false, org_count: 3 })).toBe("3 organizations");
    expect(reachLabel({ scope_all: false, org_count: 1 })).toBe("1 organization");
    expect(reachLabel({ scope_all: false, org_count: 1200 })).toBe("1,200 organizations");
  });

  it("does not call a scoped grant 'all' when it names none, or does not say how many", () => {
    expect(reachLabel({ scope_all: false, org_count: 0 })).toBe("No organizations");
    expect(reachLabel({ scope_all: false, org_count: null })).toBe("Some organizations");
  });
});

describe("formatDay", () => {
  it("formats a timestamp as a day", () => {
    expect(formatDay("2026-10-07T09:30:00Z", "UTC")).toBe("Oct 7, 2026");
  });

  it("uses the zone it is given — the same instant is a different day elsewhere", () => {
    expect(formatDay("2026-10-07T02:00:00Z", "UTC")).toBe("Oct 7, 2026");
    expect(formatDay("2026-10-07T02:00:00Z", "America/Los_Angeles")).toBe("Oct 6, 2026");
    expect(formatDay("2026-10-07T20:00:00Z", "Asia/Ho_Chi_Minh")).toBe("Oct 8, 2026");
  });

  it("answers null rather than 'Invalid Date' when there is no usable timestamp", () => {
    expect(formatDay(null, "UTC")).toBeNull();
    expect(formatDay("", "UTC")).toBeNull();
    expect(formatDay("not a date", "UTC")).toBeNull();
  });
});

describe("turnedOffBy", () => {
  const off = (over: Parameters<typeof recipient>[0] = {}) =>
    recipient({
      email: "ada@oxy.tech",
      enabled: false,
      updated_by: "boss@oxy.tech",
      updated_at: "2026-10-01T12:00:00Z",
      ...over
    });

  it("names who turned someone else's email off, and the day", () => {
    expect(turnedOffBy(off(), "UTC")).toBe("Turned off by boss@oxy.tech on Oct 1, 2026");
  });

  /**
   * The condition the line exists for. Under a row someone switched off for themselves it
   * would say "Turned off by ada@oxy.tech" beneath the name ada@oxy.tech — and with every
   * disabled row carrying a line, the rows where it is news would no longer stand out.
   */
  it("says nothing when the person turned it off themselves", () => {
    expect(turnedOffBy(off({ updated_by: "ada@oxy.tech" }), "UTC")).toBeNull();
  });

  it("knows the same person however the address was typed", () => {
    expect(turnedOffBy(off({ updated_by: "Ada@Oxy.Tech" }), "UTC")).toBeNull();
    expect(turnedOffBy(off({ updated_by: " ada@oxy.tech " }), "UTC")).toBeNull();
  });

  it("says nothing while the email is on, whoever last changed it", () => {
    // Someone else turned it off and back on: there is nothing to explain.
    expect(turnedOffBy(off({ enabled: true }), "UTC")).toBeNull();
  });

  it("says nothing when nobody is recorded as having changed it", () => {
    expect(turnedOffBy(off({ updated_by: null }), "UTC")).toBeNull();
    expect(turnedOffBy(off({ updated_by: "" }), "UTC")).toBeNull();
  });

  it("still names the person when the day is unknown", () => {
    expect(turnedOffBy(off({ updated_at: null }), "UTC")).toBe("Turned off by boss@oxy.tech");
    expect(turnedOffBy(off({ updated_at: "garbage" }), "UTC")).toBe("Turned off by boss@oxy.tech");
  });
});
