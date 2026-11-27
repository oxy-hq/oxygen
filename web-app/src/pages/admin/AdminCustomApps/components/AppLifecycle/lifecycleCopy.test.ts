import { describe, expect, it } from "vitest";
import { shortBuildId } from "./lifecycleCopy";

describe("shortBuildId", () => {
  it("cuts a commit-style id to 7 digits, as git shows a commit", () => {
    expect(shortBuildId("3f2a1b9c0d4e5f60718293a4b5c6d7e8f9a0b1c2")).toBe("3f2a1b9");
    expect(shortBuildId("3f2a1b9c0d4e5f60718293a4b5c6d7e8f9a0b1c2-45592377652.1")).toBe("3f2a1b9");
    expect(shortBuildId("0b0f6d5c6a524a2e9d0b3d7c2a1e5f10")).toBe("0b0f6d5");
  });

  it("shows an id a person chose as it was written", () => {
    expect(shortBuildId("storefront-1.2.0")).toBe("storefront-1.2.0");
    expect(shortBuildId("pulse-r5-02df9f7c9926")).toBe("pulse-r5-02df9f7c9926");
    expect(shortBuildId("v1.2.0")).toBe("v1.2.0");
  });
});
