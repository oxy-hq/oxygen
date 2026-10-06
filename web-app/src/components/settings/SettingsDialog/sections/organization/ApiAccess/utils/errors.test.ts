import { AxiosError, type AxiosResponse } from "axios";
import { describe, expect, it } from "vitest";
import { apiErrorCode, describeApiError, isApiErrorCode } from "./errors";

/** A failed request as axios hands it to a mutation's `onError`. */
const httpError = (status: number, data?: unknown): AxiosError =>
  new AxiosError("Request failed", "ERR_BAD_REQUEST", undefined, undefined, {
    status,
    data
  } as AxiosResponse);

const networkError = () => new AxiosError("Network Error", "ERR_NETWORK");

const FALLBACK = "Couldn't save.";

describe("apiErrorCode", () => {
  it("reads the code beside the error", () => {
    expect(apiErrorCode(httpError(409, { error: "taken", code: "name_taken" }))).toBe("name_taken");
  });

  it("is undefined without one", () => {
    expect(apiErrorCode(httpError(400, { error: "bad expiry" }))).toBeUndefined();
    expect(apiErrorCode(httpError(500, "<html>"))).toBeUndefined();
    expect(apiErrorCode(httpError(400, { code: 42 }))).toBeUndefined();
    expect(apiErrorCode(networkError())).toBeUndefined();
    expect(apiErrorCode(new Error("boom"))).toBeUndefined();
    expect(apiErrorCode(undefined)).toBeUndefined();
  });
});

describe("describeApiError", () => {
  it.each([
    ["session_required", 403, /signed-in browser/],
    ["name_taken", 409, /already exists/],
    ["repository_unresolved", 422, /Enter them below/],
    ["environment_required", 400, /requires an environment/],
    ["legacy_immutable", 409, /^Legacy API keys can only be extended or revoked by their owner/],
    ["use_service_account_routes", 409, /belongs to a service account/],
    ["revoked", 409, /has been revoked/]
  ])("maps the contract code %s to its own sentence", (code, status, expected) => {
    const message = describeApiError(httpError(status, { error: "server words", code }), FALLBACK);
    expect(message).toMatch(expected);
    expect(message).not.toContain("server words");
  });

  it("prefers a known code over the status it arrived with", () => {
    // session_required is a 403, but it is not a permissions problem.
    const message = describeApiError(
      httpError(403, { error: "requires a browser session", code: "session_required" }),
      FALLBACK
    );
    expect(message).not.toMatch(/owner or admin/);
  });

  it("explains a bare 403 as a permissions problem", () => {
    expect(describeApiError(httpError(403, { error: "Forbidden" }), FALLBACK)).toMatch(
      /owner or admin/
    );
  });

  it("explains a 404 as gone or not supported yet, not as the server's 'Not found'", () => {
    expect(describeApiError(httpError(404, { error: "Not found" }), FALLBACK)).toMatch(
      /no longer exists/
    );
  });

  it("uses the server's sentence for a validation failure it has no code for", () => {
    expect(describeApiError(httpError(400, { error: "expiry is in the past" }), FALLBACK)).toBe(
      "expiry is in the past"
    );
  });

  it("uses the server's sentence for a code this build doesn't know", () => {
    expect(
      describeApiError(httpError(409, { error: "policy is in use", code: "brand_new" }), FALLBACK)
    ).toBe("policy is in use");
  });

  it("reads the older { message } body shape too", () => {
    expect(describeApiError(httpError(400, { message: "name is too long" }), FALLBACK)).toBe(
      "name is too long"
    );
  });

  it("falls back when the body says nothing usable", () => {
    expect(describeApiError(httpError(500, "<html>oops</html>"), FALLBACK)).toBe(FALLBACK);
    expect(describeApiError(httpError(500, { error: "" }), FALLBACK)).toBe(FALLBACK);
    expect(describeApiError(new Error("boom"), FALLBACK)).toBe(FALLBACK);
    expect(describeApiError(undefined, FALLBACK)).toBe(FALLBACK);
  });

  it("says so when the request never got an answer", () => {
    expect(describeApiError(networkError(), FALLBACK)).toMatch(/Couldn't reach the server/);
  });
});

describe("isApiErrorCode", () => {
  it("matches only the exact code", () => {
    const err = httpError(422, { error: "unresolved", code: "repository_unresolved" });
    expect(isApiErrorCode(err, "repository_unresolved")).toBe(true);
    expect(isApiErrorCode(err, "environment_required")).toBe(false);
    expect(isApiErrorCode(new Error("boom"), "repository_unresolved")).toBe(false);
  });
});
