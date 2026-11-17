import { describe, expect, it } from "vitest";
import type { WorkspaceCompileRow } from "@/services/api/compiles";
import { compileState, compilesSentence } from "./compileState";

const row = (over: Partial<WorkspaceCompileRow> = {}): WorkspaceCompileRow => ({
  workspace_id: "w1",
  workspace_name: "pokehouse-oxy",
  org_name: "pokehouse-dev",
  workspace_path: null,
  current_revision_id: "r-old",
  current_status: "ready",
  current_git_sha: "abc",
  latest_revision_id: "r-old",
  latest_status: "ready",
  latest_started_at: "2026-11-17T00:00:00Z",
  last_ready_at: "2026-11-17T00:00:00Z",
  revision_count: 5,
  ready_count: 5,
  failed_count: 0,
  current_is_latest_ready: true,
  ...over
});

describe("compileState", () => {
  it("calls a workspace failing when its latest run failed, however good its current revision", () => {
    // The dev case: serving a ready revision from 18 days ago, every compile since failed.
    expect(compileState(row({ latest_status: "failed", failed_count: 52 }))).toBe("failing");
  });
  it("calls one behind when a newer good revision is not promoted", () => {
    expect(compileState(row({ current_is_latest_ready: false }))).toBe("behind");
  });
  it("calls one compiling while a run is in flight", () => {
    expect(compileState(row({ latest_status: "compiling" }))).toBe("compiling");
  });
  it("is up to date otherwise, and has no revision when none is current", () => {
    expect(compileState(row())).toBe("ok");
    expect(compileState(row({ current_revision_id: null, latest_status: null }))).toBe("none");
  });
});

describe("compilesSentence", () => {
  it("counts what is wrong against what is listed", () => {
    expect(
      compilesSentence([
        row({ latest_status: "failed" }),
        row({ current_is_latest_ready: false }),
        row()
      ])
    ).toBe(
      "1 failing to compile, and 1 with a newer good revision not yet promoted, of 3 workspaces."
    );
  });
  it("says so when every workspace compiles cleanly", () => {
    expect(compilesSentence([row(), row()])).toBe("All 2 workspaces compile cleanly.");
  });
});
