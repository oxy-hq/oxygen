import { describe, expect, it } from "vitest";
import {
  checkBlockerLine,
  gateLine,
  gatesFor,
  overrideTitle,
  productionGates,
  secondPersonConsequence
} from "./productionGates";

describe("productionGates", () => {
  it("holds a promoting publish to every gate Promote to production meets", () => {
    for (const rule of [false, true]) {
      expect(gatesFor("promoting_publish", rule)).toEqual(gatesFor("promote", rule));
    }
    expect(gatesFor("promote", false)).toEqual([
      "staging_never_served",
      "validation_not_passed",
      "required_secrets_unset",
      "schedule_removal_unconfirmed"
    ]);
  });

  it("adds the second-person gate only while the app's rule is on", () => {
    expect(gatesFor("promote", false)).not.toContain("same_person_promote");
    expect(gatesFor("promote", true)).toContain("same_person_promote");
    expect(gatesFor("promoting_publish", true)).toContain("same_person_promote");
  });

  it("keeps a rollback the emergency path: secrets only warn, a second person only off-history", () => {
    const rollback = gatesFor("rollback", true);
    expect(rollback).not.toContain("same_person_promote");
    expect(rollback).not.toContain("required_secrets_unset");
    expect(rollback).toContain("schedule_removal_unconfirmed");
    const secrets = productionGates(true).find((g) => g.code === "required_secrets_unset");
    expect(secrets && gateLine(secrets)).toContain("a rollback only warns");
    const fourEyes = productionGates(true).find((g) => g.code === "same_person_promote");
    expect(fourEyes && gateLine(fourEyes)).toContain(
      "a rollback only to a build production never served"
    );
  });

  it("says what the rule does in the state it is in", () => {
    expect(secondPersonConsequence(true)).toMatch(/^On:/);
    expect(secondPersonConsequence(true)).toContain("refused every time");
    expect(secondPersonConsequence(true)).toContain("production served before stays open");
    expect(secondPersonConsequence(true)).toContain("never served is refused like a promote");
    expect(secondPersonConsequence(false)).toMatch(/^Off:/);
    expect(secondPersonConsequence(false)).toContain("may promote it");
  });
});

describe("productionGates — a build's checks, with the org's gate on", () => {
  const ON = { seen_required: false };

  it("adds nothing while the gate is off", () => {
    for (const rule of [false, true]) {
      expect(productionGates(rule, null)).toEqual(productionGates(rule));
    }
    expect(gatesFor("promote", false, null)).not.toContain("checks_not_passed");
  });

  it("holds a promote and a promoting publish on the checks, and never a rollback", () => {
    for (const path of ["promote", "promoting_publish"] as const) {
      expect(gatesFor(path, false, ON).slice(-2)).toEqual(["checks_not_passed", "checks_running"]);
    }
    expect(gatesFor("rollback", false, ON)).toEqual(gatesFor("rollback", false));
    const failed = productionGates(false, ON).find((g) => g.code === "checks_not_passed");
    expect(failed && gateLine(failed)).toBe(
      "has a required check that did not pass (a rollback is exempt)"
    );
  });

  it("adds the seen gate only while the app asks for it", () => {
    expect(gatesFor("promote", false, ON)).not.toContain("changes_not_seen");
    expect(gatesFor("promote", false, { seen_required: true })).toContain("changes_not_seen");
    expect(productionGates(true, { seen_required: true })).toHaveLength(8);
  });

  it("words a check blocker for the confirm, and no other code", () => {
    expect(checkBlockerLine("checks_not_passed", ["Every screen opens"])).toBe(
      "Required check did not pass: Every screen opens"
    );
    expect(checkBlockerLine("checks_not_passed", [])).toBe("Required check did not pass");
    expect(checkBlockerLine("checks_running", ["A", "B"])).toBe(
      "Required checks still running: A, B"
    );
    expect(checkBlockerLine("changes_not_seen", ["4 screens"])).toBe(
      "Nobody has seen what changed: 4 screens"
    );
    expect(checkBlockerLine("required_secrets_unset", ["x"])).toBeNull();
  });

  it("titles the override step by the gravest blocker standing", () => {
    expect(overrideTitle(["checks_running", "checks_not_passed"])).toBe(
      "Promote past a failed check"
    );
    expect(overrideTitle(["checks_running", "changes_not_seen"])).toBe(
      "Promote past a running check"
    );
    expect(overrideTitle(["changes_not_seen"])).toBe("Promote past unseen changes");
  });
});
