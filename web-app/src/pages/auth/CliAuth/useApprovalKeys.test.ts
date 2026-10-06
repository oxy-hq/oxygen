import { describe, expect, it } from "vitest";
import { isCancelKey } from "./useApprovalKeys";

const key = (over: Partial<Parameters<typeof isCancelKey>[0]> = {}) => ({
  key: "Escape",
  isTrusted: true,
  repeat: false,
  ...over
});

describe("isCancelKey", () => {
  it("is an Escape the person pressed, once", () => {
    expect(isCancelKey(key())).toBe(true);
  });

  it("is not an Escape a script dispatched, nor one from a held key", () => {
    expect(isCancelKey(key({ isTrusted: false }))).toBe(false);
    expect(isCancelKey(key({ repeat: true }))).toBe(false);
  });

  it("is no other key: nothing but Escape is bound on the approval", () => {
    expect(isCancelKey(key({ key: "Enter" }))).toBe(false);
    expect(isCancelKey(key({ key: " " }))).toBe(false);
  });
});
