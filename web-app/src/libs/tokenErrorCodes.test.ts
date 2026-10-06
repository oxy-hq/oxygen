import { describe, expect, it } from "vitest";
import { isOrgApiAccessPath, isTokenErrorCode } from "./tokenErrorCodes";

describe("isTokenErrorCode", () => {
  it("knows the two codes a token route answers 403 with", () => {
    expect(isTokenErrorCode("session_required")).toBe(true);
    expect(isTokenErrorCode("standing_required")).toBe(true);
  });

  it("is false for another route's code, and for a body with none", () => {
    expect(isTokenErrorCode("subscription_required")).toBe(false);
    expect(isTokenErrorCode(undefined)).toBe(false);
    expect(isTokenErrorCode(403)).toBe(false);
    expect(isTokenErrorCode("")).toBe(false);
  });
});

describe("isOrgApiAccessPath", () => {
  it("matches every Organization → API access route", () => {
    expect(isOrgApiAccessPath("/orgs/o1/service-accounts")).toBe(true);
    expect(isOrgApiAccessPath("/orgs/o1/service-accounts/sa/tokens/t/extend")).toBe(true);
    expect(isOrgApiAccessPath("/orgs/o1/tokens")).toBe(true);
    expect(isOrgApiAccessPath("/orgs/o1/tokens?kind=personal")).toBe(true);
    expect(isOrgApiAccessPath("/orgs/o1/tokens/t/revoke-grant")).toBe(true);
    expect(isOrgApiAccessPath("orgs/o1/token-policy")).toBe(true);
  });

  it("leaves the org's other routes to the generic denial", () => {
    expect(isOrgApiAccessPath("/orgs/o1/members")).toBe(false);
    expect(isOrgApiAccessPath("/orgs/o1/tokens-legacy")).toBe(false);
    expect(isOrgApiAccessPath("/user/tokens")).toBe(false);
    expect(isOrgApiAccessPath("/orgs")).toBe(false);
  });
});
