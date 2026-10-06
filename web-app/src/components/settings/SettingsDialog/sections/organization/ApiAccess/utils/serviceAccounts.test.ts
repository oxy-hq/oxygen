import { describe, expect, it } from "vitest";
import { casualties, countLabel } from "./serviceAccounts";

describe("casualties", () => {
  it("names both counts", () => {
    expect(casualties({ token_count: 3, trust_policy_count: 2 })).toBe(
      "its 3 tokens and 2 trusted-access policies"
    );
  });

  it("uses the singular for one of each", () => {
    expect(casualties({ token_count: 1, trust_policy_count: 1 })).toBe(
      "its 1 token and 1 trusted-access policy"
    );
  });

  it("leaves out whichever the account doesn't have", () => {
    expect(casualties({ token_count: 2, trust_policy_count: 0 })).toBe("its 2 tokens");
    expect(casualties({ token_count: 0, trust_policy_count: 4 })).toBe(
      "its 4 trusted-access policies"
    );
  });

  it("is null when nothing else stops working", () => {
    expect(casualties({ token_count: 0, trust_policy_count: 0 })).toBeNull();
  });
});

describe("countLabel", () => {
  it("says None rather than 0", () => {
    expect(countLabel(0, "token", "tokens")).toBe("None");
  });

  it("pluralises", () => {
    expect(countLabel(1, "token", "tokens")).toBe("1 token");
    expect(countLabel(5, "policy", "policies")).toBe("5 policies");
  });
});
