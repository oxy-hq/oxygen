// @vitest-environment jsdom
// A browser: there is a `document`, so an instant with no `timeZone` is shown on the viewer's
// own clock (decision 65). The viewer's zone is fixed at Los Angeles for this file, so the
// results do not depend on the machine running it. The Function default (UTC, no `document`)
// is `dates.test.ts` and `isolate.test.ts`.

import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { defaultTimeZone, formatAsOf, formatDate, formatDateTime, formatTime } from "./dates";
import { formatRelative } from "./relative";

const VIEWER = "America/Los_Angeles";
const machineZone = process.env.TZ;

beforeAll(() => {
  process.env.TZ = VIEWER;
});

afterAll(() => {
  if (machineZone === undefined) delete process.env.TZ;
  else process.env.TZ = machineZone;
});

describe("in a browser, with no timeZone passed", () => {
  it("runs with a document and the viewer's clock fixed at Los Angeles", () => {
    expect(typeof document).toBe("object");
    expect(new Date("2026-11-19T03:00:00Z").getHours()).toBe(19);
  });

  it("defaults to the viewer's own zone", () => {
    expect(defaultTimeZone()).toBe("local");
  });

  it("shows an instant on the viewer's clock", () => {
    expect(formatTime("2026-11-19T03:00:00Z")).toBe("7:00 PM");
    expect(formatDateTime("2026-11-19T03:00:00Z", { year: false })).toBe("Nov 18, 7:00 PM");
    expect(formatAsOf("2026-11-19T03:00:00Z", { now: "2026-11-19T05:00:00Z" })).toBe(
      "As of 7:00 PM"
    );
  });

  it("counts calendar days on the viewer's clock", () => {
    // Nov 17, 11:30 PM to Nov 19, 1:00 AM in Los Angeles; in UTC it is one day.
    expect(formatRelative("2026-11-18T07:30:00Z", "2026-11-19T09:00:00Z")).toBe("2d ago");
  });

  it("still shows a passed zone instead", () => {
    expect(formatTime("2026-11-19T03:00:00Z", { timeZone: "UTC" })).toBe("3:00 AM");
  });

  it("leaves a calendar date unshifted", () => {
    expect(formatDate("2026-11-19", { year: false })).toBe("Nov 19");
  });
});
