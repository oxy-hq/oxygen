import type { Node } from "@xyflow/react";

/**
 * Rebuilt nodes keep the size xyflow already measured for their id.
 *
 * xyflow builds its internal nodes from `node.measured`. A rebuilt node
 * without one is hidden until it is measured again, and loses its handle
 * bounds (so its edges) meanwhile. A burst of rebuilds, such as one per SSE
 * event, can replace each node before that happens, leaving the graph an
 * empty canvas for good (#4494). A graph whose nodes are rebuilt while on
 * screen keeps them in `useNodesState`, passes `onNodesChange`, and sets each
 * rebuild through this.
 */
export function keepMeasured<N extends Node>(prev: N[], built: N[]): N[] {
  const measured = new Map(prev.map((n) => [n.id, n.measured] as const));
  return built.map((n) => {
    const m = measured.get(n.id);
    return m ? { ...n, measured: m } : n;
  });
}
