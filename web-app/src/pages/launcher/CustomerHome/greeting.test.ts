import { describe, expect, it } from "vitest";
import { firstName, homeGreeting } from "./greeting";

const at = (hour: number) => new Date(2026, 9, 27, hour);

describe("homeGreeting", () => {
  it("greets by first name for the part of the day", () => {
    expect(homeGreeting("Priya Silva", at(9))).toBe("Good morning, Priya");
    expect(homeGreeting("Priya Silva", at(14))).toBe("Good afternoon, Priya");
    expect(homeGreeting("Priya Silva", at(20))).toBe("Good evening, Priya");
  });

  it("leaves the name out rather than greet an email address or nothing", () => {
    expect(homeGreeting("priya@northwind.test", at(9))).toBe("Good morning");
    expect(homeGreeting("   ", at(9))).toBe("Good morning");
    expect(homeGreeting(undefined, at(9))).toBe("Good morning");
    expect(firstName("Maya")).toBe("Maya");
  });
});
