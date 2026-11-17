import { describe, expect, it } from "vitest";
import type { FailureGroup } from "@/services/api/internalJobs";
import { groupQuery, groupsSentence, groupTone, spanPhrase } from "./failureGroups";

const NOW = Date.parse("2026-11-17T12:00:00Z");
const group = (over: Partial<FailureGroup> = {}): FailureGroup => ({
  task_type: "compile",
  workspace_id: "w1",
  workspace_name: "pokehouse-oxy",
  org_id: "o1",
  org_name: "pokehouse-dev",
  signature: "compile recorded # failed file(s); revision_id=…",
  latest_error:
    "compile recorded 7 failed file(s); revision_id=75fa14c4-6adb-4ee1-af01-cdba4ad72e6f",
  occurrences: 1211,
  dead: 0,
  first_seen: "2026-10-30T12:00:00Z",
  last_seen: "2026-11-17T07:00:00Z",
  latest_task_id: "t1",
  ...over
});

describe("groupTone", () => {
  it("is danger for a cause still firing, or one with dead jobs", () => {
    expect(groupTone(group(), NOW)).toBe("danger");
    expect(groupTone(group({ last_seen: "2026-11-10T00:00:00Z", dead: 2 }), NOW)).toBe("danger");
  });
  it("is warn for one that has gone quiet", () => {
    expect(groupTone(group({ last_seen: "2026-11-10T00:00:00Z" }), NOW)).toBe("warn");
  });
});

describe("groupsSentence", () => {
  it("counts causes against failures", () => {
    expect(groupsSentence([group(), group({ occurrences: 3 })], 7)).toBe(
      "2 causes behind 1,214 failures in the last 7 days."
    );
  });
  it("says so when nothing failed", () => {
    expect(groupsSentence([], 7)).toBe("Nothing has failed in the last 7 days.");
  });
});

describe("spanPhrase", () => {
  it("tells a long-running cause from a fresh one", () => {
    expect(spanPhrase(group())).toBe("over 18 days");
    expect(spanPhrase(group({ first_seen: "2026-11-17T04:00:00Z" }))).toBe("over 3h");
    expect(spanPhrase(group({ occurrences: 1 }))).toBe("once");
  });
});

describe("groupQuery", () => {
  it("searches by workspace, or by task type for a job with none", () => {
    expect(groupQuery(group())).toBe("pokehouse-oxy");
    expect(groupQuery(group({ workspace_name: null, task_type: "reaper" }))).toBe("reaper");
  });
});
