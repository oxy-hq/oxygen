import { describe, expect, it } from "vitest";
import type {
  WorkspaceHealthHistory,
  WorkspaceHealthStatus,
  WorkspaceHealthTransition
} from "@/services/api/workspaceHealth";
import { formatSpan, lengthOf, summarize, toIntervals } from "./healthIntervals";

const NOW = Date.parse("2026-10-06T12:00:00Z");
const HOUR = 3_600_000;
const DAY = 24 * HOUR;
const ago = (ms: number) => new Date(NOW - ms).toISOString();

const change = (
  msAgo: number,
  to: WorkspaceHealthStatus,
  failing: string[] = []
): WorkspaceHealthTransition => ({
  at: ago(msAgo),
  from_status: null,
  to_status: to,
  failures: failing.map((dimension) => ({ dimension, status: to }))
});

const history = (
  transitions: WorkspaceHealthTransition[],
  over: Partial<WorkspaceHealthHistory> = {}
): WorkspaceHealthHistory => ({
  window_days: 30,
  transitions,
  opening: null,
  truncated: false,
  ...over
});

describe("toIntervals", () => {
  it("runs each stretch from its change to the next, and the newest to now", () => {
    const intervals = toIntervals(
      history([
        change(2 * HOUR, "healthy"),
        change(5 * HOUR, "unhealthy", ["pipeline", "queue"]),
        change(3 * DAY, "healthy")
      ]),
      NOW
    );

    expect(intervals.map((i) => i.status)).toEqual(["healthy", "unhealthy", "healthy"]);
    expect(intervals[0].end).toBeNull();
    expect(lengthOf(intervals[0], NOW)).toBe(2 * HOUR);
    expect(lengthOf(intervals[1], NOW)).toBe(3 * HOUR);
    expect(intervals[1].failures).toEqual(["pipeline", "queue"]);
    expect(intervals.every((i) => !i.beganBeforeWindow)).toBe(true);
  });

  // A workspace that went down forty days ago and is still down has no change
  // inside a thirty-day window. Its stretch has to appear anyway — and be
  // thirty days long, not forty: the length is time spent inside the window.
  it("cuts the stretch the window opened in at the window's edge", () => {
    const intervals = toIntervals(
      history([], { opening: change(40 * DAY, "unhealthy", ["smoke_test"]) }),
      NOW
    );

    expect(intervals).toHaveLength(1);
    expect(intervals[0].beganBeforeWindow).toBe(true);
    expect(lengthOf(intervals[0], NOW)).toBe(30 * DAY);
    expect(intervals[0].failures).toEqual(["smoke_test"]);
  });

  it("ends the opening stretch at the first change inside the window", () => {
    const intervals = toIntervals(
      history([change(10 * DAY, "healthy")], { opening: change(45 * DAY, "degraded") }),
      NOW
    );
    expect(intervals.map((i) => [i.status, lengthOf(i, NOW) / DAY])).toEqual([
      ["healthy", 10],
      ["degraded", 20]
    ]);
  });

  // The change before the window did not lead to the oldest change listed:
  // the ones cut from the list came between. Joined, they would draw a single
  // twenty-nine-day "degraded" stretch over a month that flapped.
  it("gives a cut list no opening stretch", () => {
    const intervals = toIntervals(
      history([change(1 * HOUR, "healthy"), change(1 * DAY, "unhealthy")], {
        opening: change(45 * DAY, "degraded"),
        truncated: true
      }),
      NOW
    );
    expect(intervals.map((i) => i.status)).toEqual(["healthy", "unhealthy"]);
    expect(intervals.every((i) => !i.beganBeforeWindow)).toBe(true);
  });
});

describe("formatSpan", () => {
  it("gives the size of a stretch in at most two units", () => {
    expect(formatSpan(20_000)).toBe("under a minute");
    expect(formatSpan(12 * 60_000)).toBe("12m");
    expect(formatSpan(3 * HOUR)).toBe("3h");
    expect(formatSpan(3 * HOUR + 20 * 60_000)).toBe("3h 20m");
    expect(formatSpan(2 * DAY + 4 * HOUR + 59 * 60_000)).toBe("2d 4h");
    expect(formatSpan(30 * DAY)).toBe("30d");
  });
});

describe("summarize", () => {
  it("counts the times a workspace was not healthy and totals them", () => {
    const text = summarize(
      history(
        [
          change(1 * HOUR, "healthy"),
          change(2 * HOUR, "unhealthy"),
          change(2 * DAY, "healthy"),
          change(2 * DAY + 3 * HOUR, "unhealthy"),
          change(5 * DAY, "healthy"),
          change(5 * DAY + 30 * 60_000, "degraded")
        ],
        { opening: change(40 * DAY, "healthy") }
      ),
      NOW
    );
    expect(text).toBe(
      "In the last 30 days: unhealthy twice, 4h in total · degraded once, 30m in total."
    );
  });

  // Answers that must not be confused: nothing recorded, healthy all along,
  // and too many changes to total.
  it("keeps 'nothing recorded', 'healthy throughout' and 'too many to total' apart", () => {
    expect(summarize(history([]), NOW)).toBe(
      "No status change has been recorded for this workspace yet."
    );
    expect(summarize(history([], { opening: change(60 * DAY, "healthy") }), NOW)).toBe(
      "Healthy throughout the last 30 days."
    );
    const cut = summarize(
      history([change(1 * HOUR, "unhealthy"), change(2 * HOUR, "healthy")], { truncated: true }),
      NOW
    );
    expect(cut).toMatch(/more than 2 times/);
    expect(cut).not.toMatch(/in total\./);
  });

  it("counts a stretch still going on, up to now", () => {
    expect(
      summarize(
        history([change(90 * 60_000, "unhealthy")], { opening: change(40 * DAY, "healthy") }),
        NOW
      )
    ).toBe("In the last 30 days: unhealthy once, 1h 30m in total.");
  });

  // A history that starts inside the window knows nothing of the days before
  // it. A workspace whose only row is this morning's recovery was not
  // "healthy throughout the last 30 days".
  it("speaks only for the part of the window the history covers", () => {
    expect(summarize(history([change(3 * HOUR, "healthy")]), NOW)).toBe(
      "Healthy since recording began 3h ago."
    );
    expect(
      summarize(history([change(1 * HOUR, "healthy"), change(2 * DAY, "unhealthy")]), NOW)
    ).toBe("Since recording began 2d ago: unhealthy once, 1d 23h in total.");
  });
});
