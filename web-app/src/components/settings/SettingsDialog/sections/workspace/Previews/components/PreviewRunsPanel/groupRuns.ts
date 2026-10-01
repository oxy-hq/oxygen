import type { PreviewRunSummary } from "@/types/workspace";

export interface PreviewRunGroup {
  run: PreviewRunSummary;
  children: PreviewRunSummary[];
}

/**
 * Nests a run under its parent only when the parent is itself present in the
 * list — a `compare`'s `parent_run_id` names its `transform_build`, which the
 * runs list does carry, so it nests. A `transform_build`'s `parent_run_id`
 * names the analyze run that queued it, which the runs list never lists, so
 * the build stays top-level rather than vanishing for want of a parent row.
 * A `procedure` run's `parent_run_id` is always `null` and is unaffected.
 * A self-referential `parent_run_id` (a run naming itself) is treated the
 * same way — top-level, not a child of itself — rather than being nested
 * away into its own (nonexistent) children and disappearing from the list.
 *
 * Preserves the newest-first order the list already answers in, both for the
 * top-level entries and for each parent's children.
 */
export function groupPreviewRuns(runs: PreviewRunSummary[]): PreviewRunGroup[] {
  const byId = new Map(runs.map((run) => [run.run_id, run]));
  const childIds = new Set<string>();
  const childrenByParent = new Map<string, PreviewRunSummary[]>();

  for (const run of runs) {
    if (!run.parent_run_id || run.parent_run_id === run.run_id || !byId.has(run.parent_run_id)) {
      continue;
    }
    childIds.add(run.run_id);
    const siblings = childrenByParent.get(run.parent_run_id) ?? [];
    siblings.push(run);
    childrenByParent.set(run.parent_run_id, siblings);
  }

  return runs
    .filter((run) => !childIds.has(run.run_id))
    .map((run) => ({ run, children: childrenByParent.get(run.run_id) ?? [] }));
}
