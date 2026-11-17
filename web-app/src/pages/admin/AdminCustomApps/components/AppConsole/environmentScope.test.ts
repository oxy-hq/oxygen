import { describe, expect, it } from "vitest";
import { environmentOption, environmentScope } from "./environmentScope";

describe("environmentScope", () => {
  it("reads production as the neutral default", () => {
    const s = environmentScope("production");
    expect(s.label).toBe("Production");
    expect(s.tone).toBe("muted");
  });

  it("flags staging and sandboxes: acting there does not touch production", () => {
    expect(environmentScope("staging")).toMatchObject({ label: "Staging", tone: "warn" });
    expect(environmentScope("dev-luong")).toMatchObject({
      label: "Sandbox dev-luong",
      tone: "warn"
    });
  });
});

describe("environmentOption", () => {
  it("names the gap where the environment is picked", () => {
    expect(environmentOption("staging", 2)).toBe("Staging · 2 secrets missing");
    expect(environmentOption("dev-luong", 1)).toBe("Sandbox dev-luong · 1 secret missing");
  });

  it("says nothing about a count that is zero or not known yet", () => {
    expect(environmentOption("production", 0)).toBe("Production");
    expect(environmentOption("production", undefined)).toBe("Production");
  });
});
