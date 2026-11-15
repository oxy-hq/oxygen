import type { MetricTree } from "@/types/metricTree";

export interface LeverConflict {
  upstream: string;
  downstream: string;
}

/**
 * Lever pairs that cannot be simulated together.
 *
 * Mirrors `oxy_airlayer_compat::lever_conflicts` deliberately, but the two no
 * longer play the same role. The Rust copy is the ENFORCING one, at three
 * points: both `/predict` handlers (`crates/api-metric-monitoring/src/metric_tree.rs`
 * and its `projects/` twin) 400 on a conflict, and the agentic analytics
 * `predict_impact` tool — which builds its own tree and never goes through
 * either handler — refuses the same set. So a caller with no browser in front
 * of it (curl, `oxyc`, an SDK integration, an agentic analytics tool, a
 * scheduled custom-app function) cannot get a confident answer out of an
 * ambiguous pinned-lever set. This copy is a pre-flight: it lets
 * `useScenario.ts` set `blocked = true` and skip the request before the round
 * trip that would otherwise come back 400. Both are checked against the same
 * case list — change one, change both.
 */
export function leverConflicts(tree: MetricTree, leverIds: string[]): LeverConflict[] {
  const fwd = new Map<string, string[]>();
  for (const edge of tree.edges) {
    const outgoing = fwd.get(edge.from);
    if (outgoing) outgoing.push(edge.to);
    else fwd.set(edge.from, [edge.to]);
  }

  const unique = [...new Set(leverIds)];
  const pinned = new Set(unique);
  const conflicts: LeverConflict[] = [];

  for (const start of unique) {
    // `seen` also makes this terminate on a cyclic tree — a malformed layer
    // must not hang the canvas.
    const seen = new Set([start]);
    const queue = [start];
    while (queue.length > 0) {
      const node = queue.shift() as string;
      for (const next of fwd.get(node) ?? []) {
        if (seen.has(next)) continue;
        seen.add(next);
        if (pinned.has(next)) conflicts.push({ upstream: start, downstream: next });
        queue.push(next);
      }
    }
  }
  return conflicts;
}
