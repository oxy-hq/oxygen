// @vitest-environment jsdom
// jsdom because extendChoice's module pulls in the API client, which reads `window.location`.
import { describe, expect, it } from "vitest";
import type { LifetimeCap } from "./accessDraft";
import {
  DAY_MS,
  DEFAULT_EXPIRY_CHOICE,
  EXPIRY_PRESETS,
  expiryInput,
  expiryOptions,
  expiryPreview,
  expiryProblem,
  fitChoiceToCap,
  lastPickableDay,
  pickableRange,
  presetAllowed
} from "./expiry";

// Local noon, so "end of the local day" maths holds in any timezone the suite runs in.
const now = new Date(2026, 9, 1, 12, 0, 0);
const cap = (days: number): LifetimeCap => ({ days, orgName: "Acme" });

describe("expiryInput", () => {
  it("defaults to 90 days and offers 7, 30, 90 and 365", () => {
    expect(DEFAULT_EXPIRY_CHOICE).toEqual({ kind: "days", days: 90 });
    expect(EXPIRY_PRESETS.map((p) => p.days)).toEqual([7, 30, 90, 365]);
  });

  it("sends a preset as expires_in_days, so the server counts from its own clock", () => {
    expect(expiryInput({ kind: "days", days: 30 })).toEqual({ expires_in_days: 30 });
  });

  it("sends No expiry as expires_at: null", () => {
    expect(expiryInput({ kind: "never" })).toEqual({ expires_at: null });
  });

  it("sends a picked date as the end of that local day", () => {
    const picked = new Date(2026, 10, 15);
    const end = new Date(2026, 10, 15, 23, 59, 59, 999);
    expect(expiryInput({ kind: "date", date: picked })).toEqual({ expires_at: end.toISOString() });
  });

  it("has nothing to send until a date is picked", () => {
    expect(expiryInput({ kind: "date" })).toBeUndefined();
    expect(expiryPreview({ kind: "date" }, now)).toBeUndefined();
  });
});

describe("expiryPreview", () => {
  it("counts a preset from now", () => {
    expect(expiryPreview({ kind: "days", days: 7 }, now)).toEqual(
      new Date(now.getTime() + 7 * DAY_MS)
    );
    expect(expiryPreview({ kind: "never" }, now)).toBeNull();
  });
});

describe("an org's max lifetime", () => {
  it("allows only the presets that fit", () => {
    expect(EXPIRY_PRESETS.filter((p) => presetAllowed(p.days, cap(30))).map((p) => p.days)).toEqual(
      [7, 30]
    );
    expect(presetAllowed(365, null)).toBe(true);
  });

  it("refuses No expiry and a preset past the cap, naming the org", () => {
    expect(expiryProblem({ kind: "never" }, cap(30), now)).toBe(
      "Acme limits tokens to 30 days, so this token needs an expiry."
    );
    expect(expiryProblem({ kind: "days", days: 90 }, cap(30), now)).toBe(
      "Acme limits tokens to 30 days. Pick a shorter expiry."
    );
    expect(expiryProblem({ kind: "days", days: 30 }, cap(30), now)).toBeNull();
  });

  it("has no opinion without a cap, or before a date is picked", () => {
    expect(expiryProblem({ kind: "never" }, null, now)).toBeNull();
    expect(expiryProblem({ kind: "date" }, cap(30), now)).toBeNull();
  });

  it("stops the date picker on the last day that ends inside the cap", () => {
    // 30 days from Oct 1 noon is Oct 31 noon. Oct 31 ends after that, so Oct 30 is the last day.
    const last = lastPickableDay(cap(30), now);
    expect([last.getFullYear(), last.getMonth(), last.getDate()]).toEqual([2026, 9, 30]);
    expect(expiryProblem({ kind: "date", date: last }, cap(30), now)).toBeNull();

    const tooLate = new Date(2026, 9, 31);
    expect(expiryProblem({ kind: "date", date: tooLate }, cap(30), now)).toMatch(/shorter expiry/);
  });

  it("bounds the picker from tomorrow to the cap, and leaves it open-ended without one", () => {
    const capped = pickableRange(cap(30), now);
    expect(capped.before.getDate()).toBe(2);
    expect(capped.after?.getDate()).toBe(30);
    expect(pickableRange(null, now).after).toBeUndefined();
  });

  it("falls back to the longest option that fits when a cap arrives", () => {
    expect(fitChoiceToCap({ kind: "days", days: 90 }, cap(30), now)).toEqual({
      kind: "days",
      days: 30
    });
    // A cap between presets is the longest thing on offer, so that is where it lands.
    expect(fitChoiceToCap({ kind: "never" }, cap(100), now)).toEqual({ kind: "days", days: 100 });
  });

  it("leaves a choice that already fits alone", () => {
    const choice = { kind: "days", days: 7 } as const;
    expect(fitChoiceToCap(choice, cap(30), now)).toBe(choice);
    expect(fitChoiceToCap(choice, null, now)).toBe(choice);
  });

  it("offers the cap itself when it falls between presets, and only then", () => {
    expect(expiryOptions(null).map((o) => o.days)).toEqual([7, 30, 90, 365]);
    expect(expiryOptions(cap(30)).map((o) => o.days)).toEqual([7, 30, 90, 365]);
    expect(expiryOptions(cap(45))).toContainEqual({ days: 45, label: "45 days" });
    expect(expiryOptions(cap(45)).map((o) => o.days)).toEqual([7, 30, 45, 90, 365]);
    expect(expiryOptions(cap(1))).toContainEqual({ days: 1, label: "1 day" });
    // Past the longest preset the cap rules nothing relative out, so it adds nothing.
    expect(expiryOptions(cap(3650)).map((o) => o.days)).toEqual([7, 30, 90, 365]);
  });

  it("falls back to the cap itself when no preset reaches it", () => {
    expect(fitChoiceToCap({ kind: "days", days: 90 }, cap(45), now)).toEqual({
      kind: "days",
      days: 45
    });
    // Even 7 days is too long: the cap is still something to pick.
    expect(fitChoiceToCap({ kind: "days", days: 7 }, cap(3), now)).toEqual({
      kind: "days",
      days: 3
    });
  });
});
