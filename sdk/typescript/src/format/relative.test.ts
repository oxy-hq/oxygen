// @vitest-environment node
// No `document`: calendar days are counted in UTC unless a `timeZone` is passed (decision 65).
import { describe, expect, it } from "vitest";
import { formatAge as inboxFormatAge } from "../custom-app/notifications";
import { MISSING } from "./numbers";
import { formatAge, formatRelative } from "./relative";

const LA = "America/Los_Angeles";
const NOW = "2026-11-19T12:00:00Z";
const secondsBefore = (seconds: number) => new Date(Date.parse(NOW) - seconds * 1000).toISOString();

describe("formatRelative, under a day", () => {
  it("says now under a minute, and for a clock a little ahead", () => {
    expect(formatRelative(secondsBefore(0), NOW)).toBe("now");
    expect(formatRelative(secondsBefore(59.9), NOW)).toBe("now");
    expect(formatRelative(secondsBefore(-300), NOW)).toBe("now");
  });

  it("counts whole minutes, then whole hours", () => {
    expect(formatRelative(secondsBefore(60), NOW)).toBe("1m ago");
    expect(formatRelative(secondsBefore(180), NOW)).toBe("3m ago");
    expect(formatRelative(secondsBefore(3599), NOW)).toBe("59m ago");
    expect(formatRelative(secondsBefore(3600), NOW)).toBe("1h ago");
    expect(formatRelative(secondsBefore(2 * 3600), NOW)).toBe("2h ago");
    expect(formatRelative(secondsBefore(86_399), NOW)).toBe("23h ago");
  });

  it("takes now as epoch milliseconds or a Date too", () => {
    expect(formatRelative(secondsBefore(180), Date.parse(NOW))).toBe("3m ago");
    expect(formatRelative(new Date(Date.parse(NOW) - 180_000), new Date(NOW))).toBe("3m ago");
  });
});

describe("formatRelative, from a day on", () => {
  it("counts calendar days, not 24-hour blocks", () => {
    expect(formatRelative("2026-11-18T11:00:00Z", NOW)).toBe("yesterday");
    // 25 hours, but two calendar days back.
    expect(formatRelative("2026-11-17T23:30:00Z", "2026-11-19T00:30:00Z")).toBe("2d ago");
    expect(formatRelative("2026-11-13T12:00:00Z", NOW)).toBe("6d ago");
  });

  it("counts the days in the zone named", () => {
    const at = "2026-11-18T07:30:00Z"; // Nov 17, 11:30 PM in Los Angeles
    const now = "2026-11-19T09:00:00Z"; // Nov 19, 1:00 AM in Los Angeles
    expect(formatRelative(at, now)).toBe("yesterday");
    expect(formatRelative(at, now, { timeZone: LA })).toBe("2d ago");
  });

  it("keeps hours across a 25-hour daylight-saving day", () => {
    // Nov 1 2026 in Los Angeles: 12:10 AM PDT to 11:50 PM PST is 24h40m, the same day.
    expect(formatRelative("2026-11-01T07:10:00Z", "2026-11-02T07:50:00Z", { timeZone: LA })).toBe(
      "24h ago"
    );
  });

  it("prints the date from a week on, with the year when it is not now's", () => {
    expect(formatRelative("2026-11-12T12:00:00Z", NOW)).toBe("Nov 12");
    expect(formatRelative("2025-12-30T12:00:00Z", NOW)).toBe("Dec 30, 2025");
  });

  it("counts days only for a calendar date", () => {
    expect(formatRelative("2026-11-19", NOW)).toBe("today");
    expect(formatRelative("2026-11-18", NOW)).toBe("yesterday");
    expect(formatRelative("2026-11-16", NOW)).toBe("3d ago");
    expect(formatRelative("2026-11-01", NOW)).toBe("Nov 1");
    // 7 PM on Nov 18 in Los Angeles: the 18th is still today there.
    expect(formatRelative("2026-11-18", "2026-11-19T03:00:00Z", { timeZone: LA })).toBe("today");
  });

  it("is the em dash when either side cannot be read", () => {
    expect(formatRelative(null, NOW)).toBe(MISSING);
    expect(formatRelative(undefined, NOW)).toBe(MISSING);
    expect(formatRelative("nonsense", NOW)).toBe(MISSING);
    expect(formatRelative(secondsBefore(60), "later")).toBe(MISSING);
  });
});

describe("formatAge, the inbox's stamp, moved here unchanged", () => {
  const now = Date.parse("2026-10-14T12:00:00Z");

  it("keeps the notifications panel's outputs", () => {
    expect(formatAge("2026-10-14T11:59:30Z", now)).toBe("now");
    expect(formatAge("2026-10-14T11:55:00Z", now)).toBe("5m");
    expect(formatAge("2026-10-14T09:00:00Z", now)).toBe("3h");
    expect(formatAge("2026-10-12T12:00:00Z", now)).toBe("2d");
    expect(formatAge("2026-09-01T12:00:00Z", now)).not.toBe("");
    expect(formatAge("not a date", now)).toBe("");
  });

  it("answers exactly as the custom-app module's formatAge does", () => {
    const stamps = [
      "2026-10-14T12:00:00Z",
      "2026-10-14T12:05:00Z",
      "2026-10-14T11:00:01Z",
      "2026-10-13T12:00:00Z",
      "2026-10-07T12:00:01Z",
      "2026-10-07T12:00:00Z",
      "2025-01-02T00:00:00Z",
      "",
      "garbage"
    ];
    for (const stamp of stamps)
      expect(formatAge(stamp, now), stamp).toBe(inboxFormatAge(stamp, now));
  });
});
