import { AxiosError, type AxiosResponse } from "axios";
import { describe, expect, it } from "vitest";
import type { SandboxApp } from "@/types/apiToken";
import {
  isStaleTokenError,
  isUnavailableAppError,
  sandboxMintErrorMessage,
  tokenErrorCode,
  tokenErrorMessage
} from "./tokenErrors";

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

describe("a sandbox agent token", () => {
  const LIMITS = { default_hours: 8, max_hours: 168, max_apps: 5 };
  const APPS: SandboxApp[] = [
    {
      id: "a1",
      org_id: "o1",
      org_slug: "acme",
      org_name: "Acme",
      slug: "store-ops",
      name: "Store Ops"
    }
  ];

  it("says a 409 `sandbox_token_fixed` is a token that can't be changed, whatever was tried", () => {
    const fixed = httpError(409, { code: "sandbox_token_fixed" });
    for (const action of ["update", "rename", "regenerate"] as const) {
      expect(tokenErrorMessage(fixed, action)).toBe(
        "A sandbox agent token can't be changed once it's created. Revoke it and create a new one instead."
      );
    }
    // Not a revocation: the row on screen is still right, so nothing is refetched for it.
    expect(isStaleTokenError(fixed)).toBe(false);
  });

  it("names the app a 404 `app_not_found` refuses, without claiming why", () => {
    const refused = httpError(404, { code: "app_not_found", app_id: "a1" });
    expect(sandboxMintErrorMessage(refused, APPS, LIMITS)).toBe(
      "Oxygen couldn't find Store Ops in Acme for you. It may be gone, or you may no longer build apps for that organization. Pick other apps and try again."
    );
    expect(isUnavailableAppError(refused)).toBe(true);
  });

  it("still says something useful when the refused app is none it knows", () => {
    for (const body of [{ code: "app_not_found", app_id: "zz" }, { code: "app_not_found" }]) {
      expect(sandboxMintErrorMessage(httpError(404, body), APPS, LIMITS)).toBe(
        "One of the apps is no longer available to you. Pick other apps and try again."
      );
    }
  });

  it("tells the person at the oxyc approval to run oxyc again, since they can't pick there", () => {
    const refused = httpError(404, { code: "app_not_found", app_id: "a1" });
    expect(sandboxMintErrorMessage(refused, APPS, LIMITS, "cli")).toBe(
      "Oxygen couldn't find Store Ops in Acme for you. It may be gone, or you may no longer build apps for that organization. Run oxyc again without it."
    );
    expect(sandboxMintErrorMessage(httpError(500), APPS, LIMITS, "cli")).toBe(
      "Couldn't approve the request. Try again, or run the oxyc command again for a new link."
    );
  });

  it("words a 400 `invalid_sandbox_token` by the limits, not the server's sentence", () => {
    const invalid = httpError(400, { code: "invalid_sandbox_token", message: "apps: too many" });
    expect(sandboxMintErrorMessage(invalid, APPS, LIMITS)).toBe(
      "Oxygen refused that request. A sandbox agent token names 1 to 5 apps and lasts 1 to 168 hours."
    );
    expect(isUnavailableAppError(invalid)).toBe(false);
  });

  it("passes on the server's sentence for a 400 `exceeds_policy`, in the dialog and at the oxyc approval", () => {
    // The body as the server sends it: the sentence is `error`, and there is no `message`.
    const capped = httpError(400, {
      error:
        "the expiry is past the 3-day token lifetime an organization this token reaches allows",
      code: "exceeds_policy",
      max_lifetime_days: 3
    });
    for (const surface of ["dialog", "cli"] as const) {
      expect(sandboxMintErrorMessage(capped, APPS, LIMITS, surface)).toBe(
        "the expiry is past the 3-day token lifetime an organization this token reaches allows"
      );
    }
    // Never the personal token's advice: a sandbox agent token has a lifetime, not an expiry date.
    expect(sandboxMintErrorMessage(capped, APPS, LIMITS)).not.toMatch(/Pick an earlier expiry/);
    // A body with the code and no sentence still fails as that surface does.
    const bare = httpError(400, { code: "exceeds_policy" });
    expect(sandboxMintErrorMessage(bare, APPS, LIMITS)).toBe(
      "Couldn't create the sandbox agent token. Try again."
    );
    expect(sandboxMintErrorMessage(bare, APPS, LIMITS, "cli")).toBe(
      "Couldn't approve the request. Try again, or run the oxyc command again for a new link."
    );
  });

  it("reads the server's sentence from `error`, which every token refusal carries", () => {
    // An unknown `kind`, a bad challenge or hostname: 400 with `error` and no `code`.
    expect(
      sandboxMintErrorMessage(
        httpError(400, { error: "'hostname' must be 1 to 255 characters" }),
        APPS,
        LIMITS,
        "cli"
      )
    ).toBe("'hostname' must be 1 to 255 characters");
  });

  it("says a mint needs a browser session, and falls back plainly for anything else", () => {
    expect(
      sandboxMintErrorMessage(httpError(403, { code: "session_required" }), APPS, LIMITS)
    ).toMatch(/needs a browser session/);
    expect(
      sandboxMintErrorMessage(httpError(400, { message: "name too long" }), APPS, LIMITS)
    ).toBe("name too long");
    expect(sandboxMintErrorMessage(httpError(500), APPS, LIMITS)).toBe(
      "Couldn't create the sandbox agent token. Try again."
    );
    // A bare 404 is not read as "a workspace is unavailable": that is a personal token's 404.
    expect(sandboxMintErrorMessage(httpError(404), APPS, LIMITS)).not.toMatch(/workspace/);
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
