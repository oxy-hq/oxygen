import { describe, expect, it } from "vitest";
import { isSubmitChord } from "./submitChord";

const key = (over: Partial<Parameters<typeof isSubmitChord>[0]> = {}) => ({
  key: "Enter",
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  repeat: false,
  isComposing: false,
  ...over
});

describe("isSubmitChord", () => {
  it("is Command+Enter on an Apple platform and Control+Enter elsewhere", () => {
    expect(isSubmitChord(key({ metaKey: true }), true)).toBe(true);
    expect(isSubmitChord(key({ ctrlKey: true }), true)).toBe(false);
    expect(isSubmitChord(key({ ctrlKey: true }), false)).toBe(true);
    expect(isSubmitChord(key({ metaKey: true }), false)).toBe(false);
  });

  it("is never a plain Enter, so a stray key confirms nothing", () => {
    expect(isSubmitChord(key(), true)).toBe(false);
    expect(isSubmitChord(key(), false)).toBe(false);
  });

  it("is not another key, a third modifier, a held key or a composing one", () => {
    expect(isSubmitChord(key({ key: "a", metaKey: true }), true)).toBe(false);
    expect(isSubmitChord(key({ metaKey: true, shiftKey: true }), true)).toBe(false);
    expect(isSubmitChord(key({ metaKey: true, altKey: true }), true)).toBe(false);
    expect(isSubmitChord(key({ metaKey: true, ctrlKey: true }), true)).toBe(false);
    expect(isSubmitChord(key({ metaKey: true, repeat: true }), true)).toBe(false);
    expect(isSubmitChord(key({ metaKey: true, isComposing: true }), true)).toBe(false);
  });
});
