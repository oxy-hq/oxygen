import { describe, expect, it } from "vitest";
import { appFreshnessLine } from "./appFreshness";

const now = new Date("2026-10-27T15:00:00Z");
const ago = (ms: number) => ({ published_at: new Date(now.getTime() - ms).toISOString() });
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

describe("appFreshnessLine", () => {
  it.each([
    [10_000, "Updated just now"],
    [-5 * MIN, "Updated just now"],
    [1 * MIN, "Updated 1 minute ago"],
    [45 * MIN, "Updated 45 minutes ago"],
    [1 * HOUR, "Updated 1 hour ago"],
    [5 * HOUR, "Updated 5 hours ago"],
    [1 * DAY + HOUR, "Updated yesterday"],
    [3 * DAY, "Updated 3 days ago"]
  ])("%i ms ago reads %s", (ms, line) => {
    expect(appFreshnessLine(ago(ms), now)).toBe(line);
  });

  it("names the day past a week, and the year only when it is not this one", () => {
    expect(appFreshnessLine({ published_at: "2026-10-01T12:00:00Z" }, now)).toBe("Updated Oct 1");
    expect(appFreshnessLine({ published_at: "2025-12-24T12:00:00Z" }, now)).toBe(
      "Updated Dec 24, 2025"
    );
  });

  it("says nothing rather than something made up", () => {
    expect(appFreshnessLine({ published_at: "" }, now)).toBeNull();
    expect(appFreshnessLine({ published_at: "not a date" }, now)).toBeNull();
  });
});
