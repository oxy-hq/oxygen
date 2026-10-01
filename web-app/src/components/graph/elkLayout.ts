import type { Edge, Node } from "@xyflow/react";
import ELK from "elkjs";

const elk = new ELK();

/** Default node box — the custom node renders inside this. */
const NODE_WIDTH = 220;
const NODE_HEIGHT = 64;

/**
 * Lay out a small directed hierarchy top-down with ELK's `layered` algorithm — the
 * same engine the World Model + Context Graph use, so every graph in the product
 * shares one layout brain. Returns the nodes with `position` filled in.
 */
export async function layoutTree<T extends Node>(nodes: T[], edges: Edge[]): Promise<T[]> {
  if (nodes.length === 0) return nodes;

  const graph = {
    id: "root",
    layoutOptions: {
      "elk.algorithm": "layered",
      "elk.direction": "DOWN",
      "elk.edgeRouting": "ORTHOGONAL",
      "elk.spacing.nodeNode": "36",
      "elk.layered.spacing.nodeNodeBetweenLayers": "72",
      "elk.layered.nodePlacement.strategy": "NETWORK_SIMPLEX",
      "elk.layered.crossingMinimization.strategy": "LAYER_SWEEP",
      // Multiple partners + an unmanaged bucket are disconnected subtrees; keep
      // them from overlapping.
      "elk.separateConnectedComponents": "true",
      "elk.spacing.componentComponent": "80"
    },
    children: nodes.map((n) => ({
      id: n.id,
      width: n.width ?? NODE_WIDTH,
      height: n.height ?? NODE_HEIGHT
    })),
    edges: edges.map((e) => ({ id: e.id, sources: [e.source], targets: [e.target] }))
  };

  const result = await elk.layout(graph);
  const positioned = new Map(result.children?.map((c) => [c.id, c]) ?? []);

  return nodes.map((n) => {
    const p = positioned.get(n.id);
    return p ? { ...n, position: { x: p.x ?? 0, y: p.y ?? 0 } } : n;
  });
}
