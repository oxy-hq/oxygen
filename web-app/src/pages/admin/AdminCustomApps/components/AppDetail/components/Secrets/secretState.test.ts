import { describe, expect, it } from "vitest";
import type { AppSecretEntry } from "@/types/apps";
import { inheritsFrom, isUnset } from "./secretState";

const entry = (over: Partial<AppSecretEntry>): AppSecretEntry =>
  ({
    key: "STRIPE_KEY",
    is_set: false,
    required: true,
    declared: true,
    source: "manifest",
    updated_at: null,
    ...over
  }) as AppSecretEntry;

describe("secret state", () => {
  it("a key with its own value is set, whatever it could inherit", () => {
    const e = entry({ is_set: true, inherits_production: true });
    expect(inheritsFrom(e, true)).toBeNull();
    expect(isUnset(e, true)).toBe(false);
  });

  it("a key reading another environment's value is not unset", () => {
    expect(isUnset(entry({ inherits_staging: true }), undefined)).toBe(false);
    expect(isUnset(entry({ shared_from_workspace: "ws-1" }), undefined)).toBe(false);
    expect(isUnset(entry({ inherits_production: true }), true)).toBe(false);
  });

  it("a shared key production does not hold is unset — there is nothing to inherit", () => {
    expect(inheritsFrom(entry({ inherits_production: true }), false)).toBeNull();
    expect(isUnset(entry({ inherits_production: true }), false)).toBe(true);
  });

  it("staging's value wins over production's when both are offered", () => {
    expect(inheritsFrom(entry({ inherits_staging: true, inherits_production: true }), true)).toBe(
      "staging"
    );
  });
});
