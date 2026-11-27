import { describe, expect, it } from "vitest";
import {
  checksEnvironment,
  checksLink,
  withChecksBuild,
  withChecksEnvironment
} from "./checksParams";

const params = (query: string) => new URLSearchParams(query);

describe("the Checks panel's address", () => {
  it("links to a build's staging attempt as it always did, and names production's", () => {
    expect(checksLink("9087d7b")).toEqual({ checks: "9087d7b" });
    expect(checksLink("9087d7b", "staging")).toEqual({ checks: "9087d7b" });
    expect(checksLink("60e2f19", "production")).toEqual({
      checks: "60e2f19",
      checks_env: "production"
    });
  });

  it("reads anything but production as staging", () => {
    expect(checksEnvironment(params("checks_env=production"))).toBe("production");
    expect(checksEnvironment(params(""))).toBe("staging");
    expect(checksEnvironment(params("checks_env=dev-mars"))).toBe("staging");
  });

  it("opens another build on its staging attempt, keeping the rest of the address", () => {
    const from = params("env=staging&checks=old&checks_env=production");
    expect(withChecksBuild(from, "new").toString()).toBe("env=staging&checks=new");
    expect(withChecksBuild(from, null).toString()).toBe("env=staging");
    // The address it was given is left as it was.
    expect(from.get("checks")).toBe("old");
  });

  it("switches the attempt of the build it shows", () => {
    const from = params("checks=9087d7b");
    const production = withChecksEnvironment(from, "production");
    expect(production.toString()).toBe("checks=9087d7b&checks_env=production");
    expect(withChecksEnvironment(production, "staging").toString()).toBe("checks=9087d7b");
  });
});
