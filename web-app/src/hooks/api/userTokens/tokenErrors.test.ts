import { AxiosError, type AxiosResponse } from "axios";
import { describe, expect, it } from "vitest";
import { isStaleTokenError, tokenErrorCode, tokenErrorMessage } from "./tokenErrors";

const httpError = (status: number, data: unknown = {}) =>
  new AxiosError("Request failed", "ERR", undefined, undefined, {
    status,
    data
  } as AxiosResponse);

describe("tokenErrorCode", () => {
  it("reads the code from the error body, and nothing from anything else", () => {
    expect(tokenErrorCode(httpError(409, { error: "x", code: "revoked" }))).toBe("revoked");
    expect(tokenErrorCode(httpError(409, { error: "x" }))).toBeUndefined();
    expect(tokenErrorCode(httpError(500, "gateway timeout"))).toBeUndefined();
    expect(tokenErrorCode(new Error("offline"))).toBeUndefined();
  });
});

describe("tokenErrorMessage", () => {
  it("names an org's lifetime cap on create and regenerate", () => {
    const capped = httpError(400, { error: "x", code: "exceeds_policy", max_lifetime_days: 30 });
    expect(tokenErrorMessage(capped, "create")).toMatch(/at most 30 days\. Pick an earlier expiry/);
    expect(tokenErrorMessage(capped, "regenerate")).toMatch(/Extend it to an earlier date/);
  });

  it("tells a 403 for missing standing from a 403 for no browser session", () => {
    expect(
      tokenErrorMessage(httpError(403, { error: "x", code: "standing_required" }), "create")
    ).toMatch(/doesn't hold staff or partner access/);
    expect(
      tokenErrorMessage(httpError(403, { error: "x", code: "session_required" }), "create")
    ).toMatch(/needs a browser session/);
  });

  it("says a 409 `revoked` is a revoked token", () => {
    expect(tokenErrorMessage(httpError(409, { error: "x", code: "revoked" }), "update")).toBe(
      "This token was revoked, so it can't be changed."
    );
  });

  it("has no legacy API key copy: these routes answer 404 for one, like any id that is no token", () => {
    // A legacy API key is not a token, so nothing here may word a refusal as if it were listed.
    for (const action of ["create", "update", "rename", "regenerate", "revoke"] as const) {
      const stray = tokenErrorMessage(
        httpError(409, { error: "x", code: "legacy_immutable" }),
        action
      );
      expect(stray).not.toMatch(/legacy/i);
    }
    expect(tokenErrorMessage(httpError(404), "revoke")).toBe("This token no longer exists");
  });

  it("reads a 404 on create or update as an unreachable org or workspace", () => {
    expect(tokenErrorMessage(httpError(404), "create")).toMatch(/no longer available to you/);
    expect(tokenErrorMessage(httpError(404), "update")).toMatch(/no longer available to you/);
  });

  it("reads a 404 on revoke or regenerate as the token being gone", () => {
    expect(tokenErrorMessage(httpError(404), "revoke")).toBe("This token no longer exists");
    expect(tokenErrorMessage(httpError(404), "regenerate")).toBe("This token no longer exists");
    expect(tokenErrorMessage(httpError(404), "rename")).toBe("This token no longer exists");
  });

  it("passes on the server's reason for a name it refuses", () => {
    expect(
      tokenErrorMessage(
        httpError(400, { error: "'name' must be at most 100 characters" }),
        "rename"
      )
    ).toBe("'name' must be at most 100 characters");
    expect(tokenErrorMessage(httpError(500), "rename")).toBe("Couldn't rename the token");
  });

  it("passes on the server's own words for a 400, with a fallback", () => {
    expect(
      tokenErrorMessage(httpError(400, { error: "all_access=false needs a grant" }), "create")
    ).toBe("all_access=false needs a grant");
    expect(tokenErrorMessage(httpError(400), "create")).toBe("Couldn't create the token");
  });

  it("falls back to a plain failure for anything else", () => {
    expect(tokenErrorMessage(httpError(500), "revoke")).toBe("Couldn't revoke the token");
    expect(tokenErrorMessage(new Error("offline"), "update")).toBe(
      "Couldn't update the token's access"
    );
  });
});

describe("isStaleTokenError", () => {
  it("is true when the row was revoked or deleted elsewhere", () => {
    expect(isStaleTokenError(httpError(409, { code: "revoked" }))).toBe(true);
    expect(isStaleTokenError(httpError(404))).toBe(true);
    expect(isStaleTokenError(httpError(409))).toBe(false);
    expect(isStaleTokenError(httpError(400))).toBe(false);
  });
});
