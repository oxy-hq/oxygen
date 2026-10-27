import { describe, expect, it } from "vitest";
import { earliestTodayOnEarth, formatInstant, isCalendarDate } from "./dates";

describe("earliestTodayOnEarth", () => {
  it("is today in UTC-12, the last zone to start each day", () => {
    // 05:00Z is still the 29th in UTC-12 (17:00) — a date of the 29th is not past anywhere.
    expect(earliestTodayOnEarth(new Date("2026-09-30T05:00:00Z"))).toBe("2026-09-29");
    expect(earliestTodayOnEarth(new Date("2026-09-30T13:00:00Z"))).toBe("2026-09-30");
  });
});

describe("isCalendarDate", () => {
  it.each([
    ["2026-09-30", true],
    ["2028-02-29", true],
    ["2026-02-29", false],
    ["2026-02-31", false],
    ["2026-13-01", false],
    ["2026-9-30", false],
    ["30/09/2026", false],
    ["", false]
  ])("%s -> %s", (value, expected) => {
    expect(isCalendarDate(value)).toBe(expected);
  });
});

describe("formatInstant", () => {
  it("renders an RFC 3339 instant as UTC minutes", () => {
    expect(formatInstant("2026-02-20T03:15:42Z")).toBe("2026-02-20 03:15 UTC");
  });

  it("returns an unparsable value unchanged rather than crashing the page", () => {
    expect(formatInstant("not a time")).toBe("not a time");
  });
});
