import { describe, expect, it } from "vitest";
import type { PromotionChecks } from "@/types/apps";
import {
  gateAfter,
  PROMOTION_SWITCHES,
  promotionGate,
  REQUIRABLE_CHECKS,
  requireCheck,
  setPromotionField
} from "./promotionChecksModel";

const DEFAULTS: PromotionChecks = {
  enabled: true,
  required: null,
  effective_required: ["functions", "loads", "design_system", "smoke"],
  seen_required: false,
  production_smoke: true,
  auto_rollback: false
};

describe("promotionGate", () => {
  it("is off without the org's flag: absent, null, or not enabled", () => {
    expect(promotionGate(undefined)).toBeNull();
    expect(promotionGate(null)).toBeNull();
    expect(promotionGate({ ...DEFAULTS, enabled: false })).toBeNull();
  });

  it("reads the defaults of an app that chose nothing", () => {
    expect(promotionGate(DEFAULTS)).toEqual({
      required: null,
      effective: ["functions", "loads", "design_system", "smoke"],
      seen_required: false,
      production_smoke: true,
      auto_rollback: false
    });
  });

  it("fills what a partial answer leaves out with the documented defaults", () => {
    const partial = { enabled: true } as PromotionChecks;
    expect(promotionGate(partial)).toEqual(promotionGate(DEFAULTS));
    const own = { enabled: true, required: ["smoke"] } as PromotionChecks;
    expect(promotionGate(own)?.effective).toEqual(["smoke"]);
  });

  it("lists the four checks an app can require, then the three other rules", () => {
    expect(REQUIRABLE_CHECKS.map((c) => c.kind)).toEqual([
      "functions",
      "loads",
      "design_system",
      "smoke"
    ]);
    expect(PROMOTION_SWITCHES.map((s) => s.label)).toEqual([
      "A person has seen what changed",
      "Smoke production after promote",
      "Roll back automatically if it fails"
    ]);
  });
});

describe("the request a switch sends", () => {
  const gate = promotionGate(DEFAULTS);
  if (!gate) throw new Error("the gate is on");

  it("turns one check off: the list the app then has, the rest as stored", () => {
    expect(requireCheck(gate, "design_system", false)).toEqual({
      required: ["functions", "loads", "smoke"],
      seen_required: false,
      production_smoke: true,
      auto_rollback: false
    });
  });

  it("turns one back on in the settings' order, keeping a kind it does not list", () => {
    const own = promotionGate({
      ...DEFAULTS,
      required: ["smoke", "audit"],
      effective_required: ["smoke", "audit"]
    });
    if (!own) throw new Error("the gate is on");
    expect(requireCheck(own, "functions", true).required).toEqual(["functions", "smoke", "audit"]);
  });

  it("flips another rule and leaves an app on the defaults on them", () => {
    expect(setPromotionField(gate, "seen_required", true)).toEqual({
      required: null,
      seen_required: true,
      production_smoke: true,
      auto_rollback: false
    });
    expect(setPromotionField(gate, "auto_rollback", true).auto_rollback).toBe(true);
    expect(setPromotionField(gate, "production_smoke", false).production_smoke).toBe(false);
  });

  it("shows where a save is going while it is in flight", () => {
    const after = gateAfter(gate, requireCheck(gate, "loads", false));
    expect(after.effective).toEqual(["functions", "design_system", "smoke"]);
    expect(gateAfter(gate, setPromotionField(gate, "seen_required", true))).toMatchObject({
      effective: gate.effective,
      seen_required: true
    });
  });
});
