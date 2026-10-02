import { describe, expect, it } from "vitest";
import { toText } from "./string";

describe("toText", () => {
  it("passes strings through and prints primitives as themselves", () => {
    expect(toText("a b")).toBe("a b");
    expect(toText(42)).toBe("42");
    expect(toText(false)).toBe("false");
    expect(toText(10n)).toBe("10");
  });

  it("is empty for null, undefined and things that have no text", () => {
    expect(toText(null)).toBe("");
    expect(toText(undefined)).toBe("");
    expect(toText(() => 1)).toBe("");
  });

  it("gives structured values their JSON, never [object Object]", () => {
    expect(toText({ a: 1 })).toBe('{"a":1}');
    expect(toText([1, "b"])).toBe('[1,"b"]');
    expect(toText(new Date("2026-01-02T03:04:05Z"))).toBe("2026-01-02T03:04:05.000Z");
  });

  it("never throws on a value JSON cannot express", () => {
    expect(toText({ n: 10n, list: [1n] })).toBe('{"n":"10","list":["1"]}');
    const cycle: Record<string, unknown> = {};
    cycle.self = cycle;
    expect(toText(cycle)).toBe("[object Object]");
    expect(toText(new Date("not a date"))).toBe("[object Date]");
  });
});
