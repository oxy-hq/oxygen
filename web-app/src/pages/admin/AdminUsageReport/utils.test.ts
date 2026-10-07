import { describe, expect, it } from "vitest";
import { highlight } from "./testFixtures";
import {
  appConsolePath,
  formatBytes,
  formatChange,
  formatPeriod,
  highlightLabel,
  highlightsByTone
} from "./utils";

describe("formatBytes", () => {
  const KB = 1024;
  const MB = KB * 1024;
  const GB = MB * 1024;

  it("counts in 1024s, not 1000s", () => {
    expect(formatBytes(1000)).toBe("1,000 B");
    expect(formatBytes(1024)).toBe("1 KB");
    expect(formatBytes(MB)).toBe("1.0 MB");
    expect(formatBytes(GB)).toBe("1.0 GB");
  });

  it("shows bytes and kilobytes whole", () => {
    expect(formatBytes(1)).toBe("1 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1023)).toBe("1,023 B");
    expect(formatBytes(1536)).toBe("2 KB");
    expect(formatBytes(12 * KB)).toBe("12 KB");
    expect(formatBytes(900 * KB)).toBe("900 KB");
  });

  it("shows megabytes and gigabytes to one decimal, always", () => {
    expect(formatBytes(3.4 * MB)).toBe("3.4 MB");
    expect(formatBytes(250 * MB)).toBe("250.0 MB");
    expect(formatBytes(3.4 * GB)).toBe("3.4 GB");
    expect(formatBytes(6.5 * GB)).toBe("6.5 GB");
  });

  it("stops at gigabytes, so the largest sizes stay in one unit", () => {
    expect(formatBytes(1024 * GB)).toBe("1,024.0 GB");
    expect(formatBytes(2.5 * 1024 * GB)).toBe("2,560.0 GB");
  });

  it("moves up a unit rather than print 1,024 of the one below", () => {
    // One byte short of each boundary. Rounding after choosing the unit would print
    // "1,024 KB" and "1,024.0 MB".
    expect(formatBytes(MB - 1)).toBe("1.0 MB");
    expect(formatBytes(GB - 1)).toBe("1.0 GB");
    // And the last value that honestly belongs to the unit below stays there.
    expect(formatBytes(1023 * KB)).toBe("1,023 KB");
    expect(formatBytes(1023.9 * MB)).toBe("1,023.9 MB");
  });

  it("says 0 B for nothing, and for a value that is not a size", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(-5)).toBe("0 B");
    expect(formatBytes(Number.NaN)).toBe("0 B");
  });
});

describe("formatPeriod", () => {
  it("shows the day before the exclusive end, not the end itself", () => {
    // Monday 00:00 UTC to the next Monday: the week runs through Sunday the 4th.
    expect(formatPeriod("2026-09-28T00:00:00Z", "2026-10-05T00:00:00Z")).toBe(
      "Sep 28 – Oct 4, 2026"
    );
  });

  it("formats in UTC whatever offset the timestamps are written in", () => {
    // The same two instants, written as US Pacific time. A local-zone format would start
    // this week on Sunday the 27th.
    expect(formatPeriod("2026-09-27T17:00:00-07:00", "2026-10-04T17:00:00-07:00")).toBe(
      "Sep 28 – Oct 4, 2026"
    );
  });

  it("names both years when the week crosses one", () => {
    expect(formatPeriod("2025-12-29T00:00:00Z", "2026-01-05T00:00:00Z")).toBe(
      "Dec 29, 2025 – Jan 4, 2026"
    );
  });

  it("answers null rather than 'Invalid Date' for a timestamp that does not parse", () => {
    expect(formatPeriod("not a date", "2026-10-05T00:00:00Z")).toBeNull();
    expect(formatPeriod("2026-09-28T00:00:00Z", "")).toBeNull();
  });
});

describe("formatChange", () => {
  it("signs a rise, a fall and no change", () => {
    expect(formatChange(8, 5)).toBe("+3");
    // U+2212, a real minus — a hyphen is narrower than the plus above it in the column.
    expect(formatChange(3, 5)).toBe("−2");
    expect(formatChange(5, 5)).toBe("0");
    expect(formatChange(0, 0)).toBe("0");
  });

  it("groups thousands like every other count on the page", () => {
    expect(formatChange(2500, 1000)).toBe("+1,500");
  });
});

describe("highlightLabel", () => {
  it("gives each kind its short name", () => {
    expect(highlightLabel("went_quiet")).toBe("Went quiet");
    expect(highlightLabel("dropping")).toBe("Fewer people");
    expect(highlightLabel("failing_functions")).toBe("Functions failing");
    expect(highlightLabel("client_errors")).toBe("Page errors");
    expect(highlightLabel("growing")).toBe("More people");
    expect(highlightLabel("first_week")).toBe("First week");
  });

  it("spells out a kind this build has not heard of rather than leaving it blank", () => {
    expect(highlightLabel("slow_functions")).toBe("Slow functions");
    expect(highlightLabel("")).toBe("Highlight");
  });
});

describe("highlightsByTone", () => {
  it("splits by tone and keeps the server's order inside each group", () => {
    const groups = highlightsByTone([
      highlight({ app_id: "a", tone: "good", kind: "growing" }),
      highlight({ app_id: "b", tone: "attention", kind: "went_quiet" }),
      highlight({ app_id: "c", tone: "idle", kind: "unused" }),
      highlight({ app_id: "d", tone: "attention", kind: "client_errors" }),
      highlight({ app_id: "e", tone: "good", kind: "first_week" })
    ]);
    expect(groups.attention.map((h) => h.app_id)).toEqual(["b", "d"]);
    expect(groups.good.map((h) => h.app_id)).toEqual(["a", "e"]);
    expect(groups.idle.map((h) => h.app_id)).toEqual(["c"]);
  });

  it("files a tone it does not know under attention instead of dropping it", () => {
    const stray = highlight({ app_id: "z", tone: "urgent" as never });
    expect(highlightsByTone([stray]).attention).toEqual([stray]);
  });
});

describe("appConsolePath", () => {
  it("points at the app's console under its organization", () => {
    expect(appConsolePath("rivermark", "store-ops")).toBe("/admin/apps/rivermark/store-ops");
  });
});
