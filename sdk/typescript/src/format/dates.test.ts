// @vitest-environment node
// No `document`: the Oxy Function path, where an instant with no `timeZone` is shown in UTC
// (decision 65). The browser default is `defaults.test.ts`, under jsdom.
import { describe, expect, it } from "vitest";
import {
  defaultTimeZone,
  formatAsOf,
  formatDataThrough,
  formatDate,
  formatDateRange,
  formatDateTime,
  formatMonthDay,
  formatMonthYear,
  formatShortMonth,
  formatTime,
  formatWeekday
} from "./dates";
import { MISSING } from "./numbers";

const LA = "America/Los_Angeles";
const NOW = "2026-11-19T20:00:00Z";
const UNREADABLE = [
  null,
  undefined,
  "",
  "nonsense",
  "2026-02-30",
  "2026-13-01",
  Number.NaN,
  new Date(Number.NaN)
];

describe("calendar dates", () => {
  it("are never moved by a time zone", () => {
    for (const timeZone of [LA, "Pacific/Kiritimati", "Pacific/Pago_Pago", "UTC", "local"]) {
      expect(formatDate("2026-11-19", { timeZone, year: false })).toBe("Nov 19");
      expect(formatDate("2026-03-01", { timeZone, year: false })).toBe("Mar 1");
      expect(formatMonthDay("2026-08-12", { timeZone })).toBe("August 12");
    }
  });

  it("print the em dash for anything unreadable", () => {
    for (const input of UNREADABLE) {
      expect(formatDate(input)).toBe(MISSING);
      expect(formatMonthDay(input)).toBe(MISSING);
      expect(formatDateTime(input)).toBe(MISSING);
      expect(formatWeekday(input)).toBe(MISSING);
      expect(formatAsOf(input, { now: NOW })).toBe(MISSING);
      expect(formatDataThrough(input, { now: NOW })).toBe(MISSING);
    }
  });
});

describe("instants and time zones", () => {
  const lateEvening = "2026-11-19T03:00:00Z"; // 7 PM the day before in Los Angeles

  it("default to UTC where there is no document (a Function)", () => {
    expect(typeof document).toBe("undefined");
    expect(defaultTimeZone()).toBe("UTC");
  });

  it("show an instant in UTC in a Function unless told otherwise", () => {
    expect(formatDate(lateEvening, { year: false })).toBe("Nov 19");
    expect(formatTime("2026-11-19T15:03:00Z")).toBe("3:03 PM");
  });

  it("show an instant in the zone named", () => {
    expect(formatDate(lateEvening, { timeZone: LA, year: false })).toBe("Nov 18");
    expect(formatDateTime(lateEvening, { timeZone: LA, year: false })).toBe("Nov 18, 7:00 PM");
    expect(formatDateTime(lateEvening, { timeZone: "Asia/Tokyo", year: false })).toBe(
      "Nov 19, 12:00 PM"
    );
    expect(formatWeekday(lateEvening, { timeZone: LA })).toBe("Wed");
  });

  it("follow the zone's daylight-saving change", () => {
    expect(formatTime("2026-03-08T09:30:00Z", { timeZone: LA })).toBe("1:30 AM");
    expect(formatTime("2026-03-08T10:30:00Z", { timeZone: LA })).toBe("3:30 AM");
  });

  it("read a date-time with no zone as UTC, never as the device's local time", () => {
    expect(formatTime("2026-11-19T15:03:00")).toBe("3:03 PM");
    expect(formatTime("2026-11-19T15:03")).toBe("3:03 PM");
    expect(formatDateTime("2026-10-20 02:35:00+00", { timeZone: LA, year: false })).toBe(
      "Oct 19, 7:35 PM"
    );
  });

  it("take epoch milliseconds and Date objects", () => {
    const t = Date.parse("2026-11-19T15:03:00Z");
    expect(formatTime(t)).toBe("3:03 PM");
    expect(formatTime(new Date(t))).toBe("3:03 PM");
  });

  it("read the device's own zone only when asked for local", () => {
    const t = Date.parse("2026-11-19T15:03:00Z");
    const d = new Date(t);
    const hour = d.getHours() % 12 || 12;
    const minute = String(d.getMinutes()).padStart(2, "0");
    expect(formatTime(t, { timeZone: "local" })).toBe(
      `${hour}:${minute} ${d.getHours() < 12 ? "AM" : "PM"}`
    );
  });

  it("refuse an unknown zone the way Intl does", () => {
    expect(() => formatDate(lateEvening, { timeZone: "Mars/Olympus_Mons" })).toThrow(RangeError);
  });
});

describe("the year", () => {
  it("is named only when it is not now's, and always without a now", () => {
    expect(formatDate("2026-11-19", { now: NOW })).toBe("Nov 19");
    expect(formatDate("2025-11-19", { now: NOW })).toBe("Nov 19, 2025");
    expect(formatDate("2026-11-19")).toBe("Nov 19, 2026");
  });

  it("can be forced either way", () => {
    expect(formatDate("2026-11-19", { now: NOW, year: true })).toBe("Nov 19, 2026");
    expect(formatDate("2025-11-19", { now: NOW, year: false })).toBe("Nov 19");
  });

  it("reads now in the same zone", () => {
    // Midnight New Year in UTC is still the old year in Los Angeles.
    expect(formatDate("2025-12-31", { now: "2026-01-01T03:00:00Z", timeZone: LA })).toBe("Dec 31");
    expect(formatDate("2025-12-31", { now: "2026-01-01T03:00:00Z" })).toBe("Dec 31, 2025");
  });
});

describe("the date forms", () => {
  it("lead with the weekday when asked, with Intl's comma", () => {
    expect(formatDate("2026-11-19", { weekday: true, year: false })).toBe("Thu, Nov 19");
    expect(formatWeekday("2026-11-19", { long: true })).toBe("Thursday");
  });

  it("label a month (the reference's monthYearLabel and shortMonthLabel)", () => {
    expect(formatMonthYear("2026-07")).toBe("July 2026");
    expect(formatMonthYear("2026-07-16")).toBe("July 2026");
    expect(formatMonthYear("2026-08-01T03:00:00Z", { timeZone: LA })).toBe("July 2026");
    // A month id is a calendar month: a zone west of UTC must not pull it into June.
    expect(formatMonthYear("2026-07", { timeZone: LA })).toBe("July 2026");
    expect(formatShortMonth("2026-07", { timeZone: LA })).toBe("Jul");
    expect(formatShortMonth("2026-07")).toBe("Jul");
    expect(formatShortMonth("2026-09")).toBe("Sep");
    for (const input of [null, "2026-13", "2026-00", "nonsense"]) {
      expect(formatMonthYear(input)).toBe(MISSING);
      expect(formatShortMonth(input)).toBe(MISSING);
    }
  });

  it("print a time with a plain space before AM or PM", () => {
    expect(formatTime("2026-11-19T00:00:00Z")).toBe("12:00 AM");
    expect(formatTime("2026-11-19T12:00:00Z")).toBe("12:00 PM");
    expect(formatTime("2026-11-19T09:05:00Z")).toBe("9:05 AM");
    expect(formatTime("2026-11-19T23:59:00Z")).not.toMatch(/[  ]/);
    expect(formatTime("2026-11-19")).toBe(MISSING);
  });

  it("join a date and a time", () => {
    expect(formatDateTime("2026-11-19T15:03:00Z", { now: NOW })).toBe("Nov 19, 3:03 PM");
    expect(formatDateTime("2025-11-19T15:03:00Z", { now: NOW })).toBe("Nov 19, 2025, 3:03 PM");
    expect(formatDateTime("2026-11-19")).toBe("Nov 19, 2026");
  });
});

describe("formatDateRange", () => {
  it("names a shared month and year once (store-ops' weekLabel)", () => {
    expect(formatDateRange("2026-09-21", "2026-09-27")).toBe("Sep 21 – 27, 2026");
    expect(formatDateRange("2026-09-28", "2026-10-04")).toBe("Sep 28 – Oct 4, 2026");
  });

  it("names both years across a new year, even in now's year", () => {
    expect(formatDateRange("2026-12-28", "2027-01-03", { now: NOW })).toBe(
      "Dec 28, 2026 – Jan 3, 2027"
    );
  });

  it("drops now's year, and prints one day once", () => {
    expect(formatDateRange("2026-09-21", "2026-09-27", { now: NOW })).toBe("Sep 21 – 27");
    expect(formatDateRange("2026-09-21", "2026-09-21")).toBe("Sep 21, 2026");
  });

  it("is the em dash when either end is unreadable", () => {
    expect(formatDateRange(null, "2026-09-27")).toBe(MISSING);
    expect(formatDateRange("2026-09-21", "soon")).toBe(MISSING);
  });
});

describe("freshness wording", () => {
  it("says As of and the time alone on now's day (decision 48)", () => {
    expect(formatAsOf("2026-11-19T15:03:00Z", { now: NOW })).toBe("As of 3:03 PM");
  });

  it("names the day, then the year, when it is not today", () => {
    expect(formatAsOf("2026-11-18T15:03:00Z", { now: NOW })).toBe("As of Nov 18, 3:03 PM");
    expect(formatAsOf("2025-11-18T15:03:00Z", { now: NOW })).toBe("As of Nov 18, 2025, 3:03 PM");
    expect(formatAsOf("2026-11-19T15:03:00Z")).toBe("As of Nov 19, 2026, 3:03 PM");
    expect(formatAsOf("2026-11-18", { now: NOW })).toBe("As of Nov 18");
  });

  it("decides today in the zone named", () => {
    const at = "2026-11-19T07:30:00Z"; // Nov 18, 11:30 PM in Los Angeles
    const now = "2026-11-19T08:30:00Z"; // Nov 19, 12:30 AM in Los Angeles
    expect(formatAsOf(at, { now })).toBe("As of 7:30 AM");
    expect(formatAsOf(at, { now, timeZone: LA })).toBe("As of Nov 18, 11:30 PM");
  });

  it("says Data through and the newest day", () => {
    expect(formatDataThrough("2026-11-18", { now: NOW })).toBe("Data through Nov 18");
    expect(formatDataThrough("2025-11-18", { now: NOW })).toBe("Data through Nov 18, 2025");
    expect(formatDataThrough("2026-11-18")).toBe("Data through Nov 18, 2026");
  });
});
