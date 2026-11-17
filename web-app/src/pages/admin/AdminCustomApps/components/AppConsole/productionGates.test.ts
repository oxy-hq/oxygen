import { describe, expect, it } from "vitest";
import { gateLine, gatesFor, productionGates, secondPersonConsequence } from "./productionGates";

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
