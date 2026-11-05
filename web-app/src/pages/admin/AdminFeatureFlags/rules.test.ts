import { describe, expect, it } from "vitest";
import type { FeatureFlag } from "@/services/api/featureFlags";
import {
  bpsToPercent,
  buildTargeting,
  draftFromFlag,
  isSplitValid,
  isUuid,
  orgMiss,
  percentToBps,
  reasonLabel,
  resolveOrgId,
  servingSummary,
  splitTotalBps,
  summarizeRules
} from "./rules";

const flag = (over: Partial<FeatureFlag> = {}): FeatureFlag => ({
  key: "new-home",
  description: "",
  default: false,
  enabled: true,
  updated_at: null,
  variants: ["off", "on"],
  unit: "org",
  per_org: true,
  client: true,
  targeting: null,
  targeting_error: null,
  ...over
});

const ORG_A = "11111111-1111-1111-1111-111111111111";
const ORG_B = "22222222-2222-2222-2222-222222222222";
const ORG_C = "33333333-3333-3333-3333-333333333333";

describe("percentToBps / bpsToPercent", () => {
  it("converts whole and fractional percents", () => {
    expect(percentToBps("50")).toBe(5000);
    expect(percentToBps("33.33")).toBe(3333);
    expect(percentToBps("0.5")).toBe(50);
    expect(percentToBps("100")).toBe(10000);
    expect(percentToBps("")).toBe(0);
  });

  it("rejects out-of-range and over-precise input", () => {
    expect(percentToBps("100.01")).toBeNull();
    expect(percentToBps("101")).toBeNull();
    expect(percentToBps("1.234")).toBeNull();
    expect(percentToBps("-1")).toBeNull();
    expect(percentToBps("abc")).toBeNull();
  });

  it("round-trips 33.33 / 33.33 / 33.34", () => {
    for (const text of ["33.33", "33.34", "50", "0", "0.01", "99.99"]) {
      expect(bpsToPercent(percentToBps(text) as number)).toBe(text);
    }
  });
});

describe("split total", () => {
  it("33.33 + 33.33 + 33.34 is exactly 100", () => {
    const p = { a: "33.33", b: "33.33", c: "33.34" };
    expect(splitTotalBps(p)).toBe(10000);
    expect(isSplitValid(p)).toBe(true);
  });

  it("33.33 x 3 is 99.99, not valid", () => {
    expect(isSplitValid({ a: "33.33", b: "33.33", c: "33.33" })).toBe(false);
  });

  it("is not valid when over 100 or any input is invalid", () => {
    expect(isSplitValid({ a: "60", b: "50" })).toBe(false);
    expect(splitTotalBps({ a: "50", b: "x" })).toBeNull();
    expect(isSplitValid({ a: "50", b: "x" })).toBe(false);
  });
});

describe("summarizeRules", () => {
  it("is Global with no rules or when the flag is global-only", () => {
    expect(summarizeRules(flag())).toBe("Global");
    expect(
      summarizeRules(
        flag({
          per_org: false,
          targeting: { orgs: {}, users: {}, fallthrough: { type: "variant", variant: "on" } }
        })
      )
    ).toBe("Global");
  });

  it("names a single variant for everyone", () => {
    const f = flag({
      targeting: { orgs: {}, users: {}, fallthrough: { type: "variant", variant: "on" } }
    });
    expect(summarizeRules(f)).toBe("Everyone: on");
  });

  it("counts org overrides next to a percentage rollout", () => {
    const f = flag({
      targeting: {
        orgs: { [ORG_A]: "on", [ORG_B]: "on", [ORG_C]: "off" },
        users: {},
        fallthrough: { type: "split", weights: { off: 9000, on: 1000 } }
      }
    });
    expect(summarizeRules(f)).toBe("3 orgs · 10% on");
  });

  it("shows a multi-variant split with names", () => {
    const f = flag({
      variants: ["control", "treatment"],
      targeting: {
        orgs: {},
        users: { [ORG_A]: "treatment" },
        fallthrough: { type: "split", weights: { control: 5000, treatment: 5000 } }
      }
    });
    expect(summarizeRules(f)).toBe("1 user · 50% treatment");
    const three = flag({
      variants: ["control", "a", "b"],
      targeting: {
        orgs: {},
        users: {},
        fallthrough: { type: "split", weights: { control: 5000, a: 2500, b: 2500 } }
      }
    });
    expect(summarizeRules(three)).toBe("50/25/25 control·a·b");
  });

  it("says the rest when overrides sit beside one variant", () => {
    const f = flag({
      targeting: {
        orgs: { [ORG_A]: "on" },
        users: {},
        fallthrough: { type: "variant", variant: "off" }
      }
    });
    expect(summarizeRules(f)).toBe("1 org · Rest: off");
  });
});

describe("draft <-> targeting", () => {
  it("starts a fresh draft from the flag default", () => {
    expect(draftFromFlag(flag()).variant).toBe("off");
    expect(draftFromFlag(flag({ default: true })).variant).toBe("on");
  });

  it("round-trips a split and keeps the salt", () => {
    const f = flag({
      variants: ["a", "b", "c"],
      targeting: {
        orgs: { [ORG_A]: "b" },
        users: {},
        salt: "s1",
        fallthrough: { type: "split", weights: { a: 3333, b: 3333, c: 3334 } }
      }
    });
    const draft = draftFromFlag(f);
    expect(draft.mode).toBe("split");
    expect(draft.percents).toEqual({ a: "33.33", b: "33.33", c: "33.34" });
    expect(buildTargeting(f, draft)).toEqual(f.targeting);
  });

  it("builds a variant fallthrough", () => {
    const f = flag();
    const draft = { ...draftFromFlag(f), variant: "on", orgs: { [ORG_B]: "off" } };
    expect(buildTargeting(f, draft)).toEqual({
      orgs: { [ORG_B]: "off" },
      users: {},
      fallthrough: { type: "variant", variant: "on" }
    });
  });
});

describe("isUuid", () => {
  it("accepts uuids and rejects other text", () => {
    expect(isUuid(ORG_A)).toBe(true);
    expect(isUuid(` ${ORG_A} `)).toBe(true);
    expect(isUuid("acme")).toBe(false);
  });
});

describe("orgMiss", () => {
  const orgs = [
    { id: ORG_B, name: "Dupe" },
    { id: ORG_C, name: "dupe" }
  ];
  const complete = { isLoading: false, isIncomplete: false };
  it("calls a name absent only once every org has loaded", () => {
    expect(orgMiss("Acme", orgs, complete)).toBe("absent");
    expect(orgMiss("Acme", orgs, { isLoading: true, isIncomplete: false })).toBe("loading");
  });
  it("says it couldn't check when a page of orgs failed", () => {
    // A failed page stops the drain, so the list reads as loaded but is partial.
    expect(orgMiss("Acme", orgs, { isLoading: false, isIncomplete: true })).toBe("unavailable");
  });
  it("names a duplicated name as ambiguous, however the list stands", () => {
    expect(orgMiss(" DUPE ", orgs, complete)).toBe("ambiguous");
    expect(orgMiss("dupe", orgs, { isLoading: true, isIncomplete: true })).toBe("ambiguous");
  });
});

describe("resolveOrgId", () => {
  const orgs = [
    { id: ORG_A, name: "Acme" },
    { id: ORG_B, name: "Dupe" },
    { id: ORG_C, name: "dupe" }
  ];
  it("takes a pasted uuid as is", () => {
    expect(resolveOrgId(ORG_B, orgs)).toBe(ORG_B);
  });
  it("matches a unique name, ignoring case", () => {
    expect(resolveOrgId("acme", orgs)).toBe(ORG_A);
  });
  it("refuses unknown or ambiguous names", () => {
    expect(resolveOrgId("nope", orgs)).toBeNull();
    expect(resolveOrgId("Dupe", orgs)).toBeNull();
  });
});

describe("servingSummary", () => {
  const rollout = {
    orgs: {},
    users: {},
    fallthrough: { type: "split" as const, weights: { off: 9000, on: 1000 } }
  };

  it("says on or off for everyone on a plain switch", () => {
    expect(servingSummary(flag({ enabled: true }))).toEqual({ lead: "On for everyone" });
    expect(servingSummary(flag({ enabled: false }))).toEqual({
      lead: "Off for everyone",
      detail: undefined
    });
  });

  it("leads with the rules while the switch is on", () => {
    expect(servingSummary(flag({ enabled: true, per_org: true, targeting: rollout }))).toEqual({
      lead: "10% on"
    });
  });

  it("says off for everyone when switched off, and keeps the rules in view", () => {
    expect(servingSummary(flag({ enabled: false, per_org: true, targeting: rollout }))).toEqual({
      lead: "Off for everyone",
      detail: "Rules kept: 10% on"
    });
  });

  it("flags unreadable rules as serving the off variant", () => {
    const s = servingSummary(flag({ enabled: true, per_org: true, targeting_error: "bad" }));
    expect(s).toEqual({ lead: "Everyone: off", detail: "Stored rules unreadable", broken: true });
  });
});

describe("reasonLabel", () => {
  it("names every reason the server sends, and passes an unknown one through", () => {
    expect(reasonLabel("STATIC")).toBe("Global switch");
    expect(reasonLabel("DISABLED")).toBe("Switched off");
    expect(reasonLabel("TARGETING_MATCH")).toBe("Override");
    expect(reasonLabel("SPLIT", 6342)).toBe("Split, bucket 6342");
    expect(reasonLabel("DEFAULT")).toBe("Everyone else");
    expect(reasonLabel("ERROR")).toBe("Error: served off");
    expect(reasonLabel("NEW_REASON")).toBe("NEW_REASON");
  });
});
