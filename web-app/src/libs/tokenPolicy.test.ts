import { AxiosError, AxiosHeaders } from "axios";
import { describe, expect, it } from "vitest";
import { exceedsPolicyMessage, policyBlockReason } from "./tokenPolicy";

const httpError = (status: number, data: unknown) =>
  new AxiosError("Request failed", "ERR_BAD_REQUEST", undefined, undefined, {
    status,
    statusText: "",
    headers: {},
    config: { headers: new AxiosHeaders() },
    data
  });

describe("policyBlockReason", () => {
  it("words both of the server's reasons", () => {
    expect(policyBlockReason("max_lifetime")).toMatch(/expiry is further out/);
    expect(policyBlockReason("all_access_disallowed")).toMatch(/all-access tokens/);
  });

  it("never shows a raw code for a reason it doesn't know", () => {
    expect(policyBlockReason("something_new")).not.toContain("something_new");
  });
});

describe("exceedsPolicyMessage", () => {
  const capped = httpError(400, {
    error: "the expiry is past the 30-day token lifetime",
    code: "exceeds_policy",
    max_lifetime_days: 30
  });

  it("names the cap from the body", () => {
    expect(exceedsPolicyMessage(capped)).toBe(
      "An organization this token reaches allows tokens to last at most 30 days. Pick an earlier expiry."
    );
  });

  it("tells a regenerate to shorten the expiry first", () => {
    expect(exceedsPolicyMessage(capped, "regenerate")).toMatch(/Extend it to an earlier date/);
  });

  it("says one day, not one days", () => {
    const one = httpError(400, { code: "exceeds_policy", max_lifetime_days: 1 });
    expect(exceedsPolicyMessage(one)).toContain("at most 1 day.");
  });

  it("is silent for every other error", () => {
    expect(exceedsPolicyMessage(httpError(400, { error: "bad expiry" }))).toBeUndefined();
    expect(exceedsPolicyMessage(httpError(409, { code: "revoked" }))).toBeUndefined();
    expect(exceedsPolicyMessage(new Error("boom"))).toBeUndefined();
  });
});
