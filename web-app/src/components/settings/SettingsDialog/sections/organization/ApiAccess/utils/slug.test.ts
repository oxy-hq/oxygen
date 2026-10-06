import { describe, expect, it } from "vitest";
import {
  SERVICE_ACCOUNT_NAME_MAX,
  serviceAccountHandle,
  serviceAccountNameError,
  toSlugInput
} from "./slug";

describe("toSlugInput", () => {
  it("lowercases and turns spaces and underscores into hyphens", () => {
    expect(toSlugInput("Release Bot")).toBe("release-bot");
    expect(toSlugInput("nightly_etl")).toBe("nightly-etl");
  });

  it("drops what a slug can't hold and collapses repeated hyphens", () => {
    expect(toSlugInput("deploy!@#er")).toBe("deployer");
    expect(toSlugInput("a  -  b")).toBe("a-b");
  });

  it("keeps a trailing hyphen so the next word can be typed", () => {
    expect(toSlugInput("release ")).toBe("release-");
  });

  it("stops at the length limit", () => {
    expect(toSlugInput("a".repeat(80))).toHaveLength(SERVICE_ACCOUNT_NAME_MAX);
  });
});

describe("serviceAccountNameError", () => {
  it.each(["deployer", "release-bot", "ci2", "a1-b2-c3"])("accepts %s", (name) => {
    expect(serviceAccountNameError(name)).toBeNull();
  });

  it("asks for a name when empty", () => {
    expect(serviceAccountNameError("")).toBe("Give the account a name.");
  });

  it("refuses a single character", () => {
    expect(serviceAccountNameError("a")).toMatch(/at least 2/);
  });

  it("refuses a name over the limit", () => {
    expect(serviceAccountNameError("a".repeat(SERVICE_ACCOUNT_NAME_MAX + 1))).toMatch(/at most/);
  });

  it.each(["1deployer", "-deployer"])("refuses %s for not starting with a letter", (name) => {
    expect(serviceAccountNameError(name)).toBe("Start with a lowercase letter.");
  });

  it("refuses a trailing hyphen with its own message", () => {
    expect(serviceAccountNameError("deployer-")).toMatch(/not a hyphen/);
  });

  it.each(["Deployer", "dep loyer", "dep_loyer", "dep--loyer", "dep.loyer"])(
    "refuses %s",
    (name) => {
      expect(serviceAccountNameError(name)).not.toBeNull();
    }
  );

  it("refuses a name another account already has, whatever its case", () => {
    expect(serviceAccountNameError("deployer", ["Deployer"])).toMatch(/already exists/);
    expect(serviceAccountNameError("deployer", ["publisher"])).toBeNull();
  });
});

describe("serviceAccountHandle", () => {
  it("joins the org slug and the account name", () => {
    expect(serviceAccountHandle("acme", "deployer")).toBe("acme/deployer");
  });
});
