import { describe, expect, it } from "vitest";
import type { PreviewChecksSummary } from "@/types/workspace";
import {
  previewChecksExpandable,
  previewChecksSummaryText,
  previewChecksSummaryTone
} from "./summary";

const summary = (over: Partial<PreviewChecksSummary>): PreviewChecksSummary => ({
  status: "done",
  needs_reset: 0,
  warnings: 0,
  transforms: 0,
  ...over
});

describe("previewChecksSummaryText", () => {
  it("reads as checking while there is no analyze run yet", () => {
    expect(previewChecksSummaryText(null)).toBe("checking…");
  });

  it("reads as checking while the analysis is still running", () => {
    expect(previewChecksSummaryText(summary({ status: "pending" }))).toBe("checking…");
  });

  it("says the check itself failed", () => {
    expect(previewChecksSummaryText(summary({ status: "failed" }))).toBe("check failed");
  });

  it("says there is nothing to act on when done and clean", () => {
    expect(previewChecksSummaryText(summary({ needs_reset: 0, warnings: 0 }))).toBe(
      "no pipeline changes"
    );
  });

  it("counts resets and warnings, pluralizing warnings only", () => {
    expect(previewChecksSummaryText(summary({ needs_reset: 1, warnings: 0 }))).toBe("1 need reset");
    expect(previewChecksSummaryText(summary({ needs_reset: 0, warnings: 1 }))).toBe("1 warning");
    expect(previewChecksSummaryText(summary({ needs_reset: 2, warnings: 3 }))).toBe(
      "2 need reset · 3 warnings"
    );
  });

  it("names the transform-build count when a transform-only branch has no pipeline findings", () => {
    expect(previewChecksSummaryText(summary({ transforms: 2 }))).toBe("2 transform builds");
    expect(previewChecksSummaryText(summary({ transforms: 1 }))).toBe("1 transform build");
  });

  it("combines pipeline findings and transform builds when a branch has both", () => {
    expect(previewChecksSummaryText(summary({ needs_reset: 1, warnings: 0, transforms: 2 }))).toBe(
      "1 need reset · 2 transform builds"
    );
  });

  it("still says there is nothing to act on when neither pipelines nor transforms changed", () => {
    expect(previewChecksSummaryText(summary({ needs_reset: 0, warnings: 0, transforms: 0 }))).toBe(
      "no pipeline changes"
    );
  });
});

describe("previewChecksSummaryTone", () => {
  it("is pending for null or a pending status", () => {
    expect(previewChecksSummaryTone(null)).toBe("pending");
    expect(previewChecksSummaryTone(summary({ status: "pending" }))).toBe("pending");
  });

  it("is danger whenever anything needs a reset, even alongside warnings", () => {
    expect(previewChecksSummaryTone(summary({ needs_reset: 1, warnings: 2 }))).toBe("danger");
  });

  it("is warning for warnings alone, muted for a clean pass", () => {
    expect(previewChecksSummaryTone(summary({ needs_reset: 0, warnings: 1 }))).toBe("warning");
    expect(previewChecksSummaryTone(summary({ needs_reset: 0, warnings: 0 }))).toBe("muted");
  });
});

describe("previewChecksExpandable", () => {
  it("is false with nothing to show yet", () => {
    expect(previewChecksExpandable(null)).toBe(false);
    expect(previewChecksExpandable(summary({ status: "pending" }))).toBe(false);
  });

  it("is true once the analysis has settled, failed or not", () => {
    expect(previewChecksExpandable(summary({ status: "done" }))).toBe(true);
    expect(previewChecksExpandable(summary({ status: "failed" }))).toBe(true);
  });
});
