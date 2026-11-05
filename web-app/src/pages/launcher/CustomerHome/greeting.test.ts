import { describe, expect, it } from "vitest";
import { firstName, homeGreeting } from "./greeting";

const at = (hour: number) => new Date(2026, 9, 27, hour);

describe("homeGreeting", () => {
  it("greets by first name for the part of the day", () => {
    expect(homeGreeting("Priya Silva", at(9))).toBe("Good morning, Priya");
    expect(homeGreeting("Priya Silva", at(14))).toBe("Good afternoon, Priya");
    expect(homeGreeting("Priya Silva", at(20))).toBe("Good evening, Priya");
  });

  it("reads the part of the day in the workspace's zone, not the browser's", () => {
    // 03:32 UTC on Nov 4 is 7:32 PM on Nov 3 in Los Angeles: evening there, whatever
    // the laptop's clock says.
    const now = new Date(Date.UTC(2026, 10, 4, 3, 32));
    expect(homeGreeting("Luong Vo", now, "America/Los_Angeles")).toBe("Good evening, Luong");
    expect(homeGreeting("Luong Vo", now, "Asia/Ho_Chi_Minh")).toBe("Good morning, Luong");
    // Midnight is the morning, not hour 24.
    expect(homeGreeting(null, new Date(Date.UTC(2026, 10, 4, 0, 5)), "UTC")).toBe("Good morning");
  });

  it("falls back to the browser's hour for a zone Intl does not know", () => {
    expect(homeGreeting("Priya", at(9), "Not/AZone")).toBe("Good morning, Priya");
  });

  it("leaves the name out rather than greet an email address or nothing", () => {
    expect(homeGreeting("priya@northwind.test", at(9))).toBe("Good morning");
    expect(homeGreeting("   ", at(9))).toBe("Good morning");
    expect(homeGreeting(undefined, at(9))).toBe("Good morning");
    expect(firstName("Maya")).toBe("Maya");
  });
});
