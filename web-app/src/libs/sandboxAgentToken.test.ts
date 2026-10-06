import { describe, expect, it } from "vitest";
import type { SandboxApp } from "@/types/apiToken";
import {
  appCountProblem,
  DEFAULT_SANDBOX_LIMITS,
  findSandboxApp,
  groupSandboxApps,
  hoursProblem,
  lifetimeLabel,
  lifetimePresets,
  sandboxAgentPowers,
  sandboxAgentSummary,
  sandboxAppRef,
  sandboxApps,
  sandboxExpiry,
  sandboxLimits
} from "./sandboxAgentToken";

const app = (over: Partial<SandboxApp>): SandboxApp => ({
  id: "a1",
  org_id: "o1",
  org_slug: "acme",
  org_name: "Acme",
  slug: "store-ops",
  name: "Store Ops",
  ...over
});

const APPS = [
  app({}),
  app({ id: "a2", slug: "refunds", name: "Refunds" }),
  app({ id: "a3", org_id: "o2", org_slug: "globex", org_name: "Globex", slug: "pos", name: "POS" })
];

describe("what token-options says", () => {
  it("falls back to the contract's limits and to no apps on a server that sends neither", () => {
    expect(sandboxLimits(undefined)).toEqual({ default_hours: 8, max_hours: 168, max_apps: 5 });
    expect(sandboxLimits({})).toBe(DEFAULT_SANDBOX_LIMITS);
    expect(sandboxApps(undefined)).toEqual([]);
    expect(sandboxApps({})).toEqual([]);
  });

  it("takes the server's limits and apps when it sends them", () => {
    const limits = { default_hours: 4, max_hours: 48, max_apps: 2 };
    expect(sandboxLimits({ sandbox_agent: limits })).toBe(limits);
    expect(sandboxApps({ sandbox_apps: APPS })).toBe(APPS);
  });
});

describe("the lifetime", () => {
  it("offers 1, 8, 24, 72 and 168 hours under the contract's maximum", () => {
    expect(lifetimePresets(DEFAULT_SANDBOX_LIMITS)).toEqual([1, 8, 24, 72, 168]);
  });

  it("drops what a lower maximum refuses, and offers the maximum and the default themselves", () => {
    expect(lifetimePresets({ default_hours: 4, max_hours: 48, max_apps: 5 })).toEqual([
      1, 4, 8, 24, 48
    ]);
  });

  it("counts in days once a span is two whole days or more", () => {
    expect(lifetimeLabel(1)).toBe("1 hour");
    expect(lifetimeLabel(8)).toBe("8 hours");
    expect(lifetimeLabel(24)).toBe("24 hours");
    expect(lifetimeLabel(36)).toBe("36 hours");
    expect(lifetimeLabel(72)).toBe("3 days");
    expect(lifetimeLabel(168)).toBe("7 days");
  });

  it("refuses under an hour, over the maximum and a fraction, as the server does", () => {
    expect(hoursProblem(1, DEFAULT_SANDBOX_LIMITS)).toBeNull();
    expect(hoursProblem(168, DEFAULT_SANDBOX_LIMITS)).toBeNull();
    expect(hoursProblem(0, DEFAULT_SANDBOX_LIMITS)).toBe(
      "A sandbox agent token lasts at least 1 hour."
    );
    expect(hoursProblem(169, DEFAULT_SANDBOX_LIMITS)).toBe(
      "A sandbox agent token lasts at most 168 hours (7 days)."
    );
    expect(hoursProblem(1.5, DEFAULT_SANDBOX_LIMITS)).toBe("Enter a whole number of hours.");
    // A maximum that is no whole number of days is said once, in hours.
    expect(hoursProblem(40, { default_hours: 8, max_hours: 36, max_apps: 5 })).toBe(
      "A sandbox agent token lasts at most 36 hours."
    );
  });

  it("puts the expiry that many hours from now", () => {
    const now = new Date("2026-10-06T10:00:00Z");
    expect(sandboxExpiry(8, now).toISOString()).toBe("2026-10-06T18:00:00.000Z");
  });
});

describe("the apps", () => {
  it("needs one app and takes no more than the maximum", () => {
    expect(appCountProblem(0, DEFAULT_SANDBOX_LIMITS)).toBe("Pick at least one app.");
    expect(appCountProblem(1, DEFAULT_SANDBOX_LIMITS)).toBeNull();
    expect(appCountProblem(5, DEFAULT_SANDBOX_LIMITS)).toBeNull();
    expect(appCountProblem(6, DEFAULT_SANDBOX_LIMITS)).toBe(
      "A sandbox agent token covers at most 5 apps."
    );
  });

  it("names an app the way oxyc does, and finds it by that name whatever the case", () => {
    expect(sandboxAppRef(APPS[0])).toBe("acme/store-ops");
    expect(findSandboxApp(APPS, "acme/store-ops")).toBe(APPS[0]);
    expect(findSandboxApp(APPS, " Globex/POS ")).toBe(APPS[2]);
    // The org and the app must both match: another org's app of the same slug is not it.
    expect(findSandboxApp(APPS, "globex/store-ops")).toBeUndefined();
    expect(findSandboxApp(APPS, "store-ops")).toBeUndefined();
  });

  it("groups apps under their org, both in name order", () => {
    const groups = groupSandboxApps([APPS[2], APPS[0], APPS[1]]);
    expect(groups.map((group) => group.orgName)).toEqual(["Acme", "Globex"]);
    expect(groups[0].apps.map((each) => each.name)).toEqual(["Refunds", "Store Ops"]);
  });

  it("narrows by an app's name or slug, its org's, or the whole reference", () => {
    const names = (query: string) =>
      groupSandboxApps(APPS, query).flatMap((group) => group.apps.map((each) => each.name));
    expect(names("refund")).toEqual(["Refunds"]);
    expect(names("GLOBEX")).toEqual(["POS"]);
    expect(names("acme/store")).toEqual(["Store Ops"]);
    expect(names("  ")).toEqual(["Refunds", "Store Ops", "POS"]);
    expect(groupSandboxApps(APPS, "nothing like it")).toEqual([]);
  });
});

describe("sandboxAgentSummary", () => {
  it("says what the token can do and what it can't, about the apps as the reader sees them", () => {
    const { can, cannot } = sandboxAgentSummary("these apps");
    expect(can).toContain("up to three dev sandboxes of these apps");
    expect(cannot).toContain("production or staging");
    expect(cannot).toContain("any other app");
  });

  it("reads as two sentences that start with the act, for rows labelled Can and Cannot", () => {
    const { can, cannot } = sandboxAgentSummary("the apps you pick");
    expect(can).toBe(
      "Create up to three dev sandboxes of the apps you pick, publish into them, call their functions, run their checks, read their logs and set their secrets."
    );
    expect(cannot).toBe(
      "Reach production or staging, promote a build, read a secret's value, open the admin console or touch any other app."
    );
  });
});

describe("sandboxAgentPowers", () => {
  it("lists the same acts one per line, each starting a line of its own", () => {
    const { can, cannot } = sandboxAgentPowers("these apps");
    expect(can).toEqual([
      "Create up to three dev sandboxes of these apps",
      "Publish into them",
      "Call their functions",
      "Run their checks",
      "Read their logs",
      "Set their secrets"
    ]);
    expect(cannot).toEqual([
      "Reach production or staging",
      "Promote a build",
      "Read a secret's value",
      "Open the admin console",
      "Touch any other app"
    ]);
  });

  it("names every act the sentences name, so the two can't drift apart", () => {
    const powers = sandboxAgentPowers("these apps");
    const summary = sandboxAgentSummary("these apps");
    for (const act of powers.can) expect(summary.can.toLowerCase()).toContain(act.toLowerCase());
    for (const act of powers.cannot) {
      expect(summary.cannot.toLowerCase()).toContain(act.toLowerCase());
    }
  });
});
