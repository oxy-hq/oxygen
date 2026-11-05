/**
 * The pure half of `oxyc flags`: the rule grammar, the percent → basis-point
 * conversion, the read-modify-write merge, and how a rule set reads.
 */

import { describe, expect, it } from "vitest";
import { CliError, ExitCode } from "../util/errors.js";
import { describeFallthrough, rulesSummary } from "./flags-render.js";
import {
  applyRuleEdits,
  type FeatureFlag,
  hasEdits,
  parseAssignment,
  parseSplit,
  type RuleEdits,
  type Targeting
} from "./flags-rules.js";

const ACME = "11111111-1111-4111-8111-111111111111";
const GLOBEX = "22222222-2222-4222-8222-222222222222";
const ALICE = "33333333-3333-4333-8333-333333333333";

function flag(over: Partial<FeatureFlag> = {}): FeatureFlag {
  return {
    key: "checkout_v2",
    description: "",
    default: false,
    enabled: true,
    updated_at: null,
    variants: ["control", "treatment"],
    unit: "org",
    per_org: true,
    client: true,
    targeting: null,
    targeting_error: null,
    ...over
  };
}

const none: RuleEdits = { orgs: {}, users: {} };

function usage(fn: () => unknown): CliError {
  try {
    fn();
  } catch (e) {
    expect(e).toBeInstanceOf(CliError);
    expect((e as CliError).code).toBe(ExitCode.USAGE);
    return e as CliError;
  }
  throw new Error("expected a usage error");
}

describe("parseAssignment", () => {
  it("splits at the first =", () => {
    expect(parseAssignment("acme=treatment", "--set-org")).toEqual({
      id: "acme",
      variant: "treatment"
    });
  });
  it("an empty variant is a removal", () => {
    expect(parseAssignment("acme=", "--set-org")).toEqual({ id: "acme", variant: null });
  });
  it("refuses a value with no id or no =", () => {
    usage(() => parseAssignment("acme", "--set-org"));
    usage(() => parseAssignment("=on", "--set-org"));
  });
});

describe("parseSplit", () => {
  it("converts percentages to basis points", () => {
    expect(parseSplit("control=50,treatment=50")).toEqual({ control: 5000, treatment: 5000 });
    expect(parseSplit("a=90, b=10")).toEqual({ a: 9000, b: 1000 });
  });
  it("accepts two decimals", () => {
    expect(parseSplit("a=33.34,b=33.33,c=33.33")).toEqual({ a: 3334, b: 3333, c: 3333 });
  });
  it("accepts a single variant at 100", () => {
    expect(parseSplit("treatment=100")).toEqual({ treatment: 10000 });
  });
  it("rejects a total other than 100", () => {
    expect(usage(() => parseSplit("a=50,b=40")).message).toContain("90%");
    usage(() => parseSplit("a=60,b=50"));
  });
  it("rejects shares finer than a basis point instead of rounding them", () => {
    usage(() => parseSplit("a=33.333,b=33.333,c=33.334"));
  });
  it("rejects malformed pieces and repeats", () => {
    usage(() => parseSplit(""));
    usage(() => parseSplit("a=fifty,b=50"));
    usage(() => parseSplit("a,b"));
    usage(() => parseSplit("a=50,a=50"));
    usage(() => parseSplit("a=-10,b=110"));
  });
});

describe("applyRuleEdits", () => {
  it("starts a flag with no rules from what it serves today", () => {
    const next = applyRuleEdits(flag({ enabled: true }), {
      ...none,
      orgs: { [ACME]: "treatment" }
    });
    expect(next).toEqual({
      users: {},
      orgs: { [ACME]: "treatment" },
      fallthrough: { type: "variant", variant: "treatment" }
    });
    const off = applyRuleEdits(flag({ enabled: false }), {
      ...none,
      orgs: { [ACME]: "treatment" }
    });
    expect(off?.fallthrough).toEqual({ type: "variant", variant: "control" });
  });

  it("adds one override without touching the rest", () => {
    const current: Targeting = {
      users: { [ALICE]: "treatment" },
      orgs: { [ACME]: "treatment" },
      fallthrough: { type: "split", weights: { control: 5000, treatment: 5000 } },
      salt: "s1"
    };
    const next = applyRuleEdits(flag({ targeting: current }), {
      ...none,
      orgs: { [GLOBEX]: "control" }
    });
    expect(next).toEqual({
      ...current,
      orgs: { [ACME]: "treatment", [GLOBEX]: "control" }
    });
    // The input is not mutated.
    expect(Object.keys(current.orgs)).toEqual([ACME]);
  });

  it("replaces an existing override and removes one with null", () => {
    const current: Targeting = {
      users: {},
      orgs: { [ACME]: "treatment", [GLOBEX]: "treatment" },
      fallthrough: { type: "variant", variant: "control" }
    };
    const next = applyRuleEdits(flag({ targeting: current }), {
      ...none,
      orgs: { [ACME]: "control", [GLOBEX]: null }
    });
    expect(next?.orgs).toEqual({ [ACME]: "control" });
  });

  it("--everyone and --split set only the fallthrough", () => {
    const current: Targeting = {
      users: {},
      orgs: { [ACME]: "treatment" },
      fallthrough: { type: "variant", variant: "control" }
    };
    expect(
      applyRuleEdits(flag({ targeting: current }), { ...none, everyone: "treatment" })
    ).toEqual({
      ...current,
      fallthrough: { type: "variant", variant: "treatment" }
    });
    expect(
      applyRuleEdits(flag({ targeting: current }), {
        ...none,
        split: { control: 7000, treatment: 3000 }
      })?.fallthrough
    ).toEqual({ type: "split", weights: { control: 7000, treatment: 3000 } });
    usage(() => applyRuleEdits(flag(), { ...none, everyone: "on", split: { control: 10000 } }));
  });

  it("--clear gives null, and only alone", () => {
    expect(applyRuleEdits(flag(), { ...none, clear: true })).toBeNull();
    usage(() => applyRuleEdits(flag(), { ...none, clear: true, orgs: { [ACME]: "control" } }));
  });

  it("--file replaces wholesale, and only alone", () => {
    const rules: Targeting = {
      users: {},
      orgs: {},
      fallthrough: { type: "split", weights: { control: 9000, treatment: 1000 } }
    };
    expect(applyRuleEdits(flag(), { ...none, replace: rules })).toEqual(rules);
    usage(() => applyRuleEdits(flag(), { ...none, replace: rules, everyone: "control" }));
  });

  it("checks variants against the flag", () => {
    expect(
      usage(() => applyRuleEdits(flag(), { ...none, orgs: { [ACME]: "on" } })).message
    ).toContain("control, treatment");
    usage(() => applyRuleEdits(flag(), { ...none, everyone: "nope" }));
    usage(() => applyRuleEdits(flag(), { ...none, split: { control: 5000, nope: 5000 } }));
    usage(() =>
      applyRuleEdits(flag(), {
        ...none,
        replace: { users: {}, orgs: {}, fallthrough: { type: "split", weights: { control: 5000 } } }
      })
    );
  });

  it("wants uuids for the ids, and a fallthrough in a file", () => {
    usage(() => applyRuleEdits(flag(), { ...none, users: { "not-a-uuid": "control" } }));
    usage(() => applyRuleEdits(flag(), { ...none, replace: { users: {}, orgs: {} } as never }));
  });
});

describe("hasEdits", () => {
  it("is false for nothing and true for any edit", () => {
    expect(hasEdits(none)).toBe(false);
    expect(hasEdits({ ...none, clear: true })).toBe(true);
    expect(hasEdits({ ...none, everyone: "off" })).toBe(true);
    expect(hasEdits({ ...none, users: { [ALICE]: null } })).toBe(true);
  });
});

describe("rendering", () => {
  it("summarises rules", () => {
    expect(rulesSummary(flag())).toBe("no rules");
    expect(rulesSummary(flag({ per_org: false }))).toBe("global only");
    expect(
      rulesSummary(
        flag({
          targeting: {
            users: { [ALICE]: "treatment" },
            orgs: { [ACME]: "treatment", [GLOBEX]: "control" },
            fallthrough: { type: "split", weights: { treatment: 5000, control: 5000 } }
          }
        })
      )
    ).toBe("2 orgs, 1 user, split control 50% / treatment 50%");
    expect(rulesSummary(flag({ targeting_error: "bad" }))).toBe("UNREADABLE RULES");
  });

  it("describes a fallthrough in the flag's variant order", () => {
    expect(describeFallthrough({ type: "variant", variant: "on" })).toBe("everyone else: on");
    expect(
      describeFallthrough({ type: "split", weights: { treatment: 3333, control: 6667 } }, [
        "control",
        "treatment"
      ])
    ).toBe("split control 66.67% / treatment 33.33%");
  });
});
