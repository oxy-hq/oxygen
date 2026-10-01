import { type NodeHandle, Position, type Edge as RFEdge, type Node as RFNode } from "@xyflow/react";
import type { MetricTree } from "@/types/metricTree";
import { GRAPH_EDGE_TYPE, NODE_HEIGHT_COLLAPSED, NODE_WIDTH } from "../components/semanticGraph";
import type { MetricMeasureData } from "./components/MetricMeasureNode";
import type { NodeRole } from "./components/nodeRoles";
import type { ScenarioNodeData } from "./scenario/nodeValue";

// Card geometry and the ELK runner are shared with the World Model — see
// `pages/ide/components/semanticGraph`.
export { layoutWithElk } from "../components/semanticGraph";

/** Static handle bounds mirroring the four <Handle>s `GraphNodeHandles` renders.
 *
 *  React Flow refuses to mount an edge until both endpoint nodes report handle
 *  bounds, which normally arrive only after a ResizeObserver measures the DOM —
 *  a race that silently drops edges on some loads. Declaring them on the node
 *  makes every node edge-ready from the first frame. Only the straight-line
 *  fallback reads x/y; ELK waypoints override it. */
const NODE_HANDLES: NodeHandle[] = [
  {
    id: "bottom-out",
    type: "source",
    position: Position.Bottom,
    x: NODE_WIDTH / 2,
    y: NODE_HEIGHT_COLLAPSED
  },
  { id: "top-in", type: "target", position: Position.Top, x: NODE_WIDTH / 2, y: 0 },
  { id: "top-out", type: "source", position: Position.Top, x: NODE_WIDTH / 2, y: 0 },
  {
    id: "bottom-in",
    type: "target",
    position: Position.Bottom,
    x: NODE_WIDTH / 2,
    y: NODE_HEIGHT_COLLAPSED
  }
];

/**
 * Convert a `MetricTree` into React Flow nodes and edges (unpositioned).
 * Component edges render solid; driver edges render dashed and accented.
 *
 * When `scenario` is supplied, every node switches to the `scenario-measure`
 * variant and carries `ScenarioNodeData` instead of the plain measure data —
 * a node the map has nothing for renders as `unreachable` rather than being
 * silently dropped from the canvas.
 */
/** The `data` payload for one node, in whichever of the two shapes applies.
 *
 *  Split out so each branch is checked against its own interface — see the
 *  note at the call site. */
function nodeData(
  node: MetricTree["nodes"][number],
  selectedId: string | null,
  roles: Map<string, NodeRole> | undefined,
  scenario: Map<string, ScenarioNodeData> | undefined
): MetricMeasureData | ScenarioNodeData {
  if (scenario) {
    return scenario.get(node.id) ?? { node, state: "unreachable" };
  }
  return {
    node,
    selected: node.id === selectedId,
    role: roles?.get(node.id) ?? "leaf"
  };
}

export function metricTreeToFlow(
  tree: MetricTree,
  selectedId: string | null,
  roles?: Map<string, NodeRole>,
  scenario?: Map<string, ScenarioNodeData>
): { nodes: RFNode[]; edges: RFEdge[] } {
  const nodes: RFNode[] = tree.nodes.map((node) => ({
    id: node.id,
    type: scenario ? "scenario-measure" : "metric-measure",
    position: { x: 0, y: 0 },
    sourcePosition: Position.Bottom,
    targetPosition: Position.Top,
    handles: NODE_HANDLES,
    // React Flow types node `data` as `Record<string, unknown>`; our node
    // components read it back through the matching cast. Interfaces have no
    // index signature, so the widening is explicit here rather than implied.
    //
    // The ternary is BOUND to its type before the cast, not inside it. An
    // `as unknown as` around the whole expression suppresses checking of both
    // branches — including the `{ node, state: "unreachable" }` fallback,
    // which is the one no test renders — so a field renamed on either data
    // type would compile and fail at runtime.
    data: nodeData(node, selectedId, roles, scenario) as unknown as Record<string, unknown>,
    width: NODE_WIDTH,
    height: NODE_HEIGHT_COLLAPSED,
    // Initial dimensions so React Flow renders the node immediately instead of
    // holding it hidden until the ResizeObserver measures it.
    initialWidth: NODE_WIDTH,
    initialHeight: NODE_HEIGHT_COLLAPSED
  }));

  const edges: RFEdge[] = tree.edges.map((edge, index) => {
    const isDriver = edge.kind === "driver";
    return {
      id: `${edge.from}->${edge.to}#${index}`,
      source: edge.from,
      target: edge.to,
      // ELK-routed orthogonal path, same router the World Model draws.
      type: GRAPH_EDGE_TYPE,
      // Deliberately never `animated`. React Flow's animated edges run
      // `dashdraw` — an infinite `stroke-dashoffset` animation — and that
      // property is not compositor-accelerated, so every frame re-rasterizes
      // the whole edge layer across the full ELK canvas. On a tree with any
      // real number of driver edges that held the GPU at 100% the entire time
      // the view was open, with nobody touching it. The dash pattern and the
      // accent stroke below already say "driver"; motion said nothing more.
      // (The World Model animates its selected edges; its graph is an order of
      // magnitude smaller. Consistency stops at the point it costs a frame.)
      data: { kind: edge.kind },
      style: {
        stroke: isDriver ? "var(--primary)" : "var(--success)",
        strokeWidth: isDriver ? 1.5 : 2,
        strokeDasharray: isDriver ? "6 4" : undefined,
        opacity: 0.7
      }
    };
  });

  return { nodes, edges };
}
