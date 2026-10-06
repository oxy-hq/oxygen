// @vitest-environment jsdom
// jsdom because ApiKeyService's module pulls in the API client, which reads `window.location`.
import { AxiosError, type AxiosResponse } from "axios";
import { describe, expect, it } from "vitest";
import type { ApiKey } from "@/types/apiKey";
import {
  extendErrorMessage,
  extendSuccessMessage,
  isStaleKeyError,
  tokenNoun
} from "./extendMessages";

const httpError = (status: number, data: unknown = {}) =>
  new AxiosError("Request failed", "ERR", undefined, undefined, {
    status,
    data
  } as AxiosResponse);

const key = (over: Partial<ApiKey>): ApiKey => ({
  id: "k1",
  name: "CI",
  created_at: "2026-01-01T00:00:00Z",
  is_active: true,
  ...over
});

describe("extendErrorMessage", () => {
  it("names the org's lifetime cap rather than the server's sentence", () => {
    const capped = httpError(400, { error: "x", code: "exceeds_policy", max_lifetime_days: 90 });
    expect(extendErrorMessage(capped, "token")).toBe(
      "An organization this token reaches allows tokens to last at most 90 days. Pick an earlier expiry."
    );
  });

  it("tells a sandbox agent token's fixed lifetime from a revocation, though both are 409", () => {
    expect(extendErrorMessage(httpError(409, { code: "sandbox_token_fixed" }), "token")).toBe(
      "A sandbox agent token can't be extended. Create a new one when it lapses."
    );
    expect(extendErrorMessage(httpError(409, { code: "revoked" }), "token")).toBe(
      "This token was revoked and can't be extended"
    );
  });

  it("maps the contract's statuses to what the person can do about them", () => {
    expect(extendErrorMessage(httpError(409))).toBe(
      "This legacy API key was revoked and can't be extended"
    );
    expect(extendErrorMessage(httpError(404))).toBe("This legacy API key no longer exists");
    expect(extendErrorMessage(httpError(403))).toBe("Extending needs a browser session");
    expect(extendErrorMessage(httpError(400, { error: "days must be 1-3650" }))).toBe(
      "days must be 1-3650"
    );
    expect(extendErrorMessage(httpError(400))).toBe("That expiry isn't valid");
    expect(extendErrorMessage(httpError(500))).toBe("Couldn't extend the legacy API key");
  });

  it("treats 404 and 409 as a stale row", () => {
    expect(isStaleKeyError(httpError(404))).toBe(true);
    expect(isStaleKeyError(httpError(409))).toBe(true);
    expect(isStaleKeyError(httpError(400))).toBe(false);
  });
});

describe("tokenNoun", () => {
  it("calls a legacy row a legacy API key and anything newer a token, never the other way", () => {
    // No `kind`: a row from the legacy `/api-keys` routes, which list legacy API keys only.
    expect(tokenNoun({})).toBe("legacy API key");
    expect(tokenNoun({ kind: "legacy_key" })).toBe("legacy API key");
    expect(tokenNoun({ kind: "personal" })).toBe("token");
    expect(tokenNoun({ kind: "service_account" })).toBe("token");
    expect(tokenNoun({ kind: "ci" })).toBe("token");
  });

  it("never words a legacy API key's failure as a token's", () => {
    for (const status of [400, 403, 404, 409, 500]) {
      expect(extendErrorMessage(httpError(status), "legacy API key")).not.toMatch(/token/i);
    }
  });

  it("names the thing in an extend failure", () => {
    expect(extendErrorMessage(httpError(409), "token")).toBe(
      "This token was revoked and can't be extended"
    );
    expect(extendErrorMessage(httpError(404), "token")).toBe("This token no longer exists");
    expect(extendErrorMessage(httpError(500), "token")).toBe("Couldn't extend the token");
  });
});

describe("extendSuccessMessage", () => {
  const future = "2099-01-30T12:00:00Z";

  it("says a lapsed key is back", () => {
    const before = key({ expires_at: "2020-01-01T00:00:00Z" });
    expect(extendSuccessMessage(before, key({ expires_at: future }))).toBe(
      '"CI" is active again until Jan 30, 2099'
    );
  });

  it("states the new expiry, or that there is none", () => {
    const before = key({ expires_at: "2098-01-01T00:00:00Z" });
    expect(extendSuccessMessage(before, key({ expires_at: future }))).toBe(
      '"CI" now expires Jan 30, 2099'
    );
    expect(extendSuccessMessage(before, key({ expires_at: undefined }))).toBe(
      '"CI" no longer expires'
    );
  });
});
