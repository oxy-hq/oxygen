import { describe, expect, it } from "vitest";
import type { MonitorPreview, PreviewFlag, SegmentPreview } from "@/types/metricAnomalies";
import { deviationPercent, previewHeadline, tally, windowPhrase } from "./monitorPreview";

const flag = (over: Partial<PreviewFlag> = {}): PreviewFlag => ({
  timestamp: "2026-10-04T00:00:00Z",
  observed: 1204,
  expected: 1571,
  lower: 1400,
  upper: 1700,
  residual: -367,
  z_score: -4,
  severity: "high",
  ...over
});

const scored = (flags: number, key = ""): SegmentPreview => ({
  dimension_key: key,
  state: "scored",
  measured_buckets: 90,
  required_buckets: 56,
  flagged: Array.from({ length: flags }, () => flag())
});
const warming = (key = ""): SegmentPreview => ({
  dimension_key: key,
  state: "warming_up",
  measured_buckets: 20,
  required_buckets: 56
});
const failed = (key = ""): SegmentPreview => ({ dimension_key: key, state: "failed", error: "x" });

const preview = (segments: SegmentPreview[], total = segments.length): MonitorPreview => ({
  window_buckets: 7,
  segments_total: total,
  segments
});

describe("windowPhrase", () => {
  it("names the window a scan of each grain scores", () => {
    expect(windowPhrase("day", 7)).toBe("the 7 most recent days");
    expect(windowPhrase("week", 1)).toBe("the most recent week");
    expect(windowPhrase("month", 1)).toBe("the most recent month");
  });
});

describe("previewHeadline", () => {
  it("says what one monitor would flag", () => {
    expect(previewHeadline(preview([scored(2)]), "day")).toEqual({
      tone: "flagged",
      text: "A scan now would flag 2 of the 7 most recent days."
    });
    // A weekly or monthly scan scores one bucket, so there is no "1 of 1".
    expect(previewHeadline({ ...preview([scored(1)]), window_buckets: 1 }, "week").text).toBe(
      "A scan now would flag the most recent week."
    );
  });

  // The three answers that are not "flagged" must not blur into each other. A
  // monitor that is not being scored, or that could not run, has not been
  // checked — only a scored one may say it found nothing.
  it("never reports an unscored or failed monitor as having found nothing", () => {
    expect(previewHeadline(preview([scored(0)]), "day")).toEqual({
      tone: "quiet",
      text: "A scan now would flag nothing in the 7 most recent days."
    });
    expect(previewHeadline(preview([warming()]), "day")).toEqual({
      tone: "warming",
      text: "Not scored yet — 20 of the 56 days of history it needs."
    });
    expect(previewHeadline(preview([failed()]), "day")).toEqual({
      tone: "failed",
      text: "This monitor could not be run."
    });
    for (const one of [warming(), failed()]) {
      expect(previewHeadline(preview([one]), "day").text).not.toMatch(/nothing/);
    }
  });

  it("does not call a fan-out with no segments a clean result", () => {
    const empty = previewHeadline(preview([], 0), "day");
    expect(empty.tone).toBe("warming");
    expect(empty.text).toBe(
      "Nothing to score — this monitor's group_by dimension has no values in its lookback window."
    );
    expect(empty.text).not.toMatch(/would flag nothing/);
  });

  it("counts a fanned-out monitor's segments by outcome, and says when it saw only some", () => {
    const some = preview([scored(1, "s=1"), scored(0, "s=2"), warming("s=3"), failed("s=4")], 15);
    expect(tally(some)).toEqual({ flagging: 1, quiet: 1, warming: 1, failed: 1 });
    expect(previewHeadline(some, "day")).toEqual({
      tone: "flagged",
      text: "Across the first 4 of 15 segments: 1 would be flagged, 1 not scored yet, 1 could not be run."
    });
    // The whole answer is a failure only when nothing ran.
    expect(previewHeadline(preview([failed("s=1"), failed("s=2")]), "day").tone).toBe("failed");
    expect(previewHeadline(preview([failed("s=1"), scored(0, "s=2")]), "day").tone).toBe("warming");
    expect(previewHeadline(preview([scored(0, "s=1"), scored(0, "s=2")]), "week")).toEqual({
      tone: "quiet",
      text: "Across 2 segments, a scan now would flag nothing in the 7 most recent weeks."
    });
  });
});

describe("deviationPercent", () => {
  it("matches the inbox's Δ%, and has no answer against an expectation of zero", () => {
    expect(deviationPercent(flag())).toBeCloseTo(-23.36, 1);
    expect(deviationPercent(flag({ observed: 12, expected: 0 }))).toBeNull();
  });
});
