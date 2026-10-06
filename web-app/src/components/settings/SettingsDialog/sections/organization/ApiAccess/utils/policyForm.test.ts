import { describe, expect, it } from "vitest";
import type { TokenPolicy } from "@/types/orgApiAccess";
import {
  DEFAULT_TOKEN_POLICY,
  formToPolicy,
  isPolicyDirty,
  MAX_LIFETIME_LIMIT,
  maxLifetimeError,
  type PolicyFormState,
  policyChangeNotes,
  policyToForm
} from "./policyForm";

const form = (over: Partial<PolicyFormState> = {}): PolicyFormState => ({
  ...policyToForm(DEFAULT_TOKEN_POLICY),
  ...over
});

describe("DEFAULT_TOKEN_POLICY", () => {
  it("matches what the server answers for an org with no policy row", () => {
    expect(DEFAULT_TOKEN_POLICY).toEqual({
      max_lifetime_days: null,
      allow_all_access_tokens: true,
      require_environment_on_trust_policies: true
    });
  });
});

describe("policyToForm / formToPolicy", () => {
  it("round-trips a policy with no lifetime limit", () => {
    expect(formToPolicy(policyToForm(DEFAULT_TOKEN_POLICY))).toEqual(DEFAULT_TOKEN_POLICY);
  });

  it("round-trips a policy with every setting changed", () => {
    const policy: TokenPolicy = {
      max_lifetime_days: 30,
      allow_all_access_tokens: false,
      require_environment_on_trust_policies: false
    };
    expect(formToPolicy(policyToForm(policy))).toEqual(policy);
  });

  it("sends null, not the remembered number, when the limit is off", () => {
    expect(formToPolicy(form({ limitLifetime: false, maxLifetimeDays: "45" }))).toMatchObject({
      max_lifetime_days: null
    });
  });

  it("sends the number as a number", () => {
    expect(formToPolicy(form({ limitLifetime: true, maxLifetimeDays: " 45 " }))).toMatchObject({
      max_lifetime_days: 45
    });
  });

  it("is null while the lifetime is invalid", () => {
    expect(formToPolicy(form({ limitLifetime: true, maxLifetimeDays: "" }))).toBeNull();
  });
});

describe("maxLifetimeError", () => {
  const withDays = (maxLifetimeDays: string) => form({ limitLifetime: true, maxLifetimeDays });

  it("ignores the field while the limit is off", () => {
    expect(maxLifetimeError(form({ limitLifetime: false, maxLifetimeDays: "abc" }))).toBeNull();
  });

  it.each(["1", "90", String(MAX_LIFETIME_LIMIT)])("accepts %s", (value) => {
    expect(maxLifetimeError(withDays(value))).toBeNull();
  });

  it("asks for a number when empty", () => {
    expect(maxLifetimeError(withDays("  "))).toBe("Enter a number of days.");
  });

  it.each(["1.5", "-3", "ninety", "1e3"])("refuses %s as not a whole number", (value) => {
    expect(maxLifetimeError(withDays(value))).toBe("Use a whole number of days.");
  });

  it("refuses zero", () => {
    expect(maxLifetimeError(withDays("0"))).toBe("Use at least 1 day.");
  });

  it("refuses more than the limit", () => {
    expect(maxLifetimeError(withDays(String(MAX_LIFETIME_LIMIT + 1)))).toMatch(/at most/);
  });
});

describe("isPolicyDirty", () => {
  it("is clean for an untouched form", () => {
    expect(isPolicyDirty(policyToForm(DEFAULT_TOKEN_POLICY), DEFAULT_TOKEN_POLICY)).toBe(false);
  });

  it("is dirty after any change", () => {
    expect(isPolicyDirty(form({ allowAllAccessTokens: false }), DEFAULT_TOKEN_POLICY)).toBe(true);
    expect(isPolicyDirty(form({ requireEnvironment: false }), DEFAULT_TOKEN_POLICY)).toBe(true);
    expect(isPolicyDirty(form({ limitLifetime: true }), DEFAULT_TOKEN_POLICY)).toBe(true);
  });

  it("ignores a typed number while the limit stays off", () => {
    expect(isPolicyDirty(form({ maxLifetimeDays: "45" }), DEFAULT_TOKEN_POLICY)).toBe(false);
  });

  it("is dirty while invalid, so the form can say why it won't save", () => {
    expect(
      isPolicyDirty(form({ limitLifetime: true, maxLifetimeDays: "" }), DEFAULT_TOKEN_POLICY)
    ).toBe(true);
  });
});

describe("policyChangeNotes", () => {
  const strict: TokenPolicy = {
    max_lifetime_days: 90,
    allow_all_access_tokens: false,
    require_environment_on_trust_policies: true
  };

  it("has nothing to say about an unchanged or invalid form", () => {
    expect(policyChangeNotes(policyToForm(strict), strict)).toEqual([]);
    expect(policyChangeNotes(form({ limitLifetime: true, maxLifetimeDays: "x" }), strict)).toEqual(
      []
    );
  });

  it("says a new lifetime limit blocks rather than revokes", () => {
    const [note] = policyChangeNotes(
      form({ limitLifetime: true, maxLifetimeDays: "30" }),
      DEFAULT_TOKEN_POLICY
    );
    expect(note).toMatch(/30 days/);
    expect(note).toMatch(/not revoked/);
  });

  it("warns when a limit gets shorter, but not when it gets longer or goes away", () => {
    expect(
      policyChangeNotes({ ...policyToForm(strict), maxLifetimeDays: "30" }, strict)
    ).toHaveLength(1);
    expect(policyChangeNotes({ ...policyToForm(strict), maxLifetimeDays: "365" }, strict)).toEqual(
      []
    );
    expect(policyChangeNotes({ ...policyToForm(strict), limitLifetime: false }, strict)).toEqual(
      []
    );
  });

  it("warns when all-access tokens are turned off, and not when turned back on", () => {
    expect(policyChangeNotes(form({ allowAllAccessTokens: false }), DEFAULT_TOKEN_POLICY)).toEqual([
      expect.stringMatching(/All-access personal tokens stop working/)
    ]);
    expect(
      policyChangeNotes({ ...policyToForm(strict), allowAllAccessTokens: true }, strict)
    ).toEqual([]);
  });

  it("warns when the environment requirement is dropped", () => {
    expect(policyChangeNotes(form({ requireEnvironment: false }), DEFAULT_TOKEN_POLICY)).toEqual([
      expect.stringMatching(/anyone who can push/)
    ]);
  });

  it("lists every consequence of a combined change", () => {
    const notes = policyChangeNotes(
      {
        limitLifetime: true,
        maxLifetimeDays: "7",
        allowAllAccessTokens: false,
        requireEnvironment: false
      },
      DEFAULT_TOKEN_POLICY
    );
    expect(notes).toHaveLength(3);
  });
});
