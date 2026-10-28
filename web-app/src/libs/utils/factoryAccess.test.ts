import { describe, expect, it } from "vitest";
import { canUseFactory } from "./factoryAccess";

describe("canUseFactory", () => {
  it("hides the Factory only on an explicit no", () => {
    expect(canUseFactory({ factory_access: false })).toBe(false);
  });

  it("shows it on a yes, and on anything that is not an explicit no", () => {
    expect(canUseFactory({ factory_access: true })).toBe(true);
    expect(canUseFactory({})).toBe(true);
    expect(canUseFactory(null)).toBe(true);
    expect(canUseFactory(undefined)).toBe(true);
  });
});
