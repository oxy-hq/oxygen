import { describe, expect, it } from "vitest";
import type { PreviewRunSummary } from "@/types/workspace";
import { groupPreviewRuns } from "./groupRuns";

const run = (over: Partial<PreviewRunSummary>): PreviewRunSummary => ({
  run_id: "run-1",
  branch: "feat/x",
  kind: "procedure",
  target_ref: "workflows/x.procedure.yml",
  revision_id: "rev-1",
  state: "finished",
  outcome: "succeeded",
  held_count: 0,
  requested_by: null,
  created_at: "2026-09-29T08:00:00Z",
  started_at: null,
  finished_at: null,
  parent_run_id: null,
  ...over
});

describe("groupPreviewRuns", () => {
  it("puts every procedure run at the top level, with no children", () => {
    const runs = [run({ run_id: "p-2" }), run({ run_id: "p-1" })];
    expect(groupPreviewRuns(runs)).toEqual([
      { run: runs[0], children: [] },
      { run: runs[1], children: [] }
    ]);
  });

  it("keeps a transform_build at the top level when its parent (the analyze run) isn't in the list", () => {
    const build = run({ run_id: "build-1", kind: "transform_build", parent_run_id: "analyze-1" });
    expect(groupPreviewRuns([build])).toEqual([{ run: build, children: [] }]);
  });

  it("nests a compare under its transform_build when the build is in the list", () => {
    const build = run({ run_id: "build-1", kind: "transform_build", parent_run_id: "analyze-1" });
    const compare = run({
      run_id: "compare-1",
      kind: "compare",
      held_count: 0,
      parent_run_id: "build-1"
    });
    // Newest-first as the list would answer: compare, then its build.
    expect(groupPreviewRuns([compare, build])).toEqual([{ run: build, children: [compare] }]);
  });

  it("treats a self-referential parent_run_id as top-level, not a vanished child of itself", () => {
    const odd = run({ run_id: "odd-1", parent_run_id: "odd-1" });
    expect(groupPreviewRuns([odd])).toEqual([{ run: odd, children: [] }]);
  });

  it("preserves order and mixes ungrouped runs alongside a build/compare pair", () => {
    const procedure = run({ run_id: "p-1" });
    const build = run({ run_id: "build-1", kind: "transform_build", parent_run_id: "analyze-1" });
    const compare = run({ run_id: "compare-1", kind: "compare", parent_run_id: "build-1" });

    expect(groupPreviewRuns([procedure, compare, build])).toEqual([
      { run: procedure, children: [] },
      { run: build, children: [compare] }
    ]);
  });
});
