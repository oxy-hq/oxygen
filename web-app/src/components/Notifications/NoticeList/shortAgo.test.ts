import { describe, expect, it } from "vitest";
import { shortAgo } from "./shortAgo";

const now = new Date("2026-11-23T12:00:00Z");
const ago = (ms: number) => new Date(now.getTime() - ms).toISOString();
const MIN = 60_000;

describe("shortAgo", () => {
  it("writes the panel's short times", () => {
    expect(shortAgo(ago(20_000), now)).toBe("Just now");
    expect(shortAgo(ago(12 * MIN), now)).toBe("12m ago");
    expect(shortAgo(ago(2 * 60 * MIN), now)).toBe("2h ago");
    expect(shortAgo(ago(30 * 60 * MIN), now)).toBe("Yesterday");
    expect(shortAgo(ago(3 * 24 * 60 * MIN), now)).toBe("3d ago");
  });

  it("names the date after a week, with the year only when it is another one", () => {
    expect(shortAgo("2026-11-05T12:00:00Z", now)).toBe("Nov 5");
    expect(shortAgo("2025-11-05T12:00:00Z", now)).toBe("Nov 5, 2025");
  });

  it("reads a clock a little ahead as now, and an unparsable time as nothing", () => {
    expect(shortAgo(new Date(now.getTime() + 5_000).toISOString(), now)).toBe("Just now");
    expect(shortAgo("not a time", now)).toBe("");
  });
});
