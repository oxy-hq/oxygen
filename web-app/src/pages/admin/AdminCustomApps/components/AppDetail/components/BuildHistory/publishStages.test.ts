import { describe, expect, it } from "vitest";
import type { PublishAttempt } from "@/types/appPublishAttempts";
import {
  failedAttempts,
  formatMs,
  landedByBuild,
  publisherLabel,
  stageSegments,
  totalMs
} from "./publishStages";

const attempt = (over: Partial<PublishAttempt> = {}): PublishAttempt => ({
  id: "a-1",
  app_build_id: "row-1",
  build_id: "ci-1",
  environment: "staging",
  outcome: "success",
  failed_stage: null,
  error: null,
  started_at: "2026-11-17T10:00:00Z",
  finished_at: "2026-11-17T10:00:02Z",
  duration_ms: 2000,
  stages: [
    { stage: "admit", ms: 10 },
    { stage: "inflate", ms: 290 },
    { stage: "store", ms: 700 }
  ],
  published_by_email: null,
  published_via: null,
  published_token_id: null,
  ...over
});

describe("formatMs", () => {
  it("labels in ms under a second, then in seconds", () => {
    expect(formatMs(0)).toBe("0ms");
    expect(formatMs(12.4)).toBe("12ms");
    expect(formatMs(999)).toBe("999ms");
    expect(formatMs(1000)).toBe("1.0s");
    expect(formatMs(1250)).toBe("1.3s");
    expect(formatMs(42_000)).toBe("42s");
  });

  it("never prints a negative or a NaN", () => {
    expect(formatMs(-5)).toBe("0ms");
    expect(formatMs(Number.NaN)).toBe("0ms");
  });
});

describe("stageSegments", () => {
  it("sizes each stage by its share of the whole, in the order they ran", () => {
    const segments = stageSegments(attempt().stages);
    expect(segments.map((s) => s.stage)).toEqual(["admit", "inflate", "store"]);
    expect(segments.map((s) => s.share)).toEqual([0.01, 0.29, 0.7]);
    expect(totalMs(attempt().stages)).toBe(1000);
  });

  it("gives a stage that took no time no share, and keeps it", () => {
    const segments = stageSegments([
      { stage: "admit", ms: 0 },
      { stage: "inflate", ms: 50 }
    ]);
    expect(segments.map((s) => [s.stage, s.share])).toEqual([
      ["admit", 0],
      ["inflate", 1]
    ]);
  });

  it("splits the bar evenly when nothing took a millisecond", () => {
    const segments = stageSegments([
      { stage: "admit", ms: 0 },
      { stage: "inflate", ms: 0 }
    ]);
    expect(segments.map((s) => s.share)).toEqual([0.5, 0.5]);
  });

  it("marks only the stage the publish failed at", () => {
    const segments = stageSegments(attempt().stages, "inflate");
    expect(segments.filter((s) => s.failed).map((s) => s.stage)).toEqual(["inflate"]);
    expect(stageSegments(attempt().stages, null).some((s) => s.failed)).toBe(false);
  });
});

describe("failedAttempts", () => {
  it("keeps the failures, in the order served", () => {
    const list = [
      attempt({ id: "new", outcome: "failed", app_build_id: null }),
      attempt({ id: "ok" }),
      attempt({ id: "old", outcome: "failed", app_build_id: null })
    ];
    expect(failedAttempts(list).map((a) => a.id)).toEqual(["new", "old"]);
    expect(failedAttempts(undefined)).toEqual([]);
  });
});

describe("landedByBuild", () => {
  it("indexes the attempts that landed by their build row, newest winning", () => {
    const byBuild = landedByBuild([
      attempt({ id: "newer", app_build_id: "row-1" }),
      attempt({ id: "older", app_build_id: "row-1" }),
      attempt({ id: "other", app_build_id: "row-2" }),
      attempt({ id: "failed", outcome: "failed", app_build_id: "row-3" }),
      attempt({ id: "reaped", app_build_id: null })
    ]);
    expect([...byBuild.entries()].map(([row, a]) => [row, a.id])).toEqual([
      ["row-1", "newer"],
      ["row-2", "other"]
    ]);
  });
});

describe("publisherLabel", () => {
  it("names the person, else the workflow, else the agent token", () => {
    expect(
      publisherLabel(attempt({ published_by_email: "dev@acme.com", published_via: "ci" }))
    ).toBe("dev@acme.com");
    expect(publisherLabel(attempt({ published_via: "github-oidc:acme/app" }))).toBe(
      "github-oidc:acme/app"
    );
    expect(publisherLabel(attempt({ published_token_id: "tok" }))).toBe("a sandbox agent token");
    expect(publisherLabel(attempt())).toBeNull();
  });
});
