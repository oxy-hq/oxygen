import type { Edge } from "@xyflow/react";
import ELK, { type ElkExtendedEdge, type ElkNode } from "elkjs";
import { type TaskNode as Node, TaskType } from "@/stores/useAutomation";
import {
  contentPadding,
  distanceBetweenHeaderAndContent,
  distanceBetweenNodes,
  headerHeight,
  loopProgressBarHeight,
  nodeBorder,
  nodePadding
} from "./constants";

const elk = new ELK();

export const createElkLayout = async (nodes: Node[], edges: Edge[]): Promise<Node[]> => {
  warnOfDanglingEdges(nodes, edges);
  const elkGraph = createElkGraph(nodes, edges);
  const layout = await elk.layout(elkGraph);
  return extractLayoutedNodes(layout, nodes);
};

/**
 * `buildElkNodes` lays out an edge only where both its ends are, so an edge to a
 * node that does not exist is left out: ELK would reject the whole graph over
 * it. Such an edge is a bug in whatever built the graph, and left out silently
 * one hid the conditional branches' broken edges for months. Say so, once.
 */
const warnOfDanglingEdges = (nodes: Node[], edges: Edge[]) => {
  const ids = new Set(nodes.map((node) => node.id));
  const dangling = edges
    .filter((edge) => !ids.has(edge.source) || !ids.has(edge.target))
    .map((edge) => edge.id);
  if (dangling.length === 0) return;
  console.warn(
    `Automation diagram: left out ${dangling.length} edge(s) to a node that does not exist: ` +
      dangling.join(", ")
  );
};

const createElkGraph = (nodes: Node[], edges: Edge[]) => {
  return {
    id: "root",
    layoutOptions: {
      "elk.algorithm": "layered",
      "elk.direction": "DOWN"
    },
    ...buildElkNodes(
      nodes.filter((node) => !node.parentId),
      nodes,
      edges
    )
  };
};

const buildElkNodes = (
  nodes: Node[],
  allNodes: Node[],
  allEdges: Edge[]
): {
  children: ElkNode[];
  edges: ElkExtendedEdge[];
} => {
  return {
    children: nodes.map((node) => {
      const childNodes = node.data.expanded ? allNodes.filter((n) => n.parentId === node.id) : [];
      const padding = calculateNodePadding(childNodes.length, node.type);
      return {
        id: node.id,
        width: node.width,
        height: node.height,
        layoutOptions: createLayoutOptions(node, padding),
        ...buildElkNodes(childNodes, allNodes, allEdges)
      };
    }),
    edges: allEdges
      .filter((edge) => {
        const source = nodes.find((n) => n.id === edge.source);
        const target = nodes.find((n) => n.id === edge.target);
        return !!source && !!target;
      })
      .map((edge) => ({
        id: edge.id,
        sources: [edge.source],
        targets: [edge.target]
      }))
  };
};

const extractLayoutedNodes = (layout: ElkNode, flatNodes: Node[]): Node[] => {
  let layoutedNodes: Node[] = [];

  if (!layout.children) return layoutedNodes;

  layout.children.forEach((node) => {
    const originalNode = flatNodes.find((n) => n.id === node.id)!;
    layoutedNodes.push({
      ...originalNode,
      position: {
        x: node.x || 0,
        y: node.y || 0
      },
      width: node.width || 0,
      height: node.height || 0
    });
    layoutedNodes = layoutedNodes.concat(extractLayoutedNodes(node, flatNodes));
  });

  return layoutedNodes;
};

const calculateNodePadding = (childCount: number, nodeType?: Node["type"]) => {
  // Loop nodes reserve `bar + header→bar gap` between the header and
  // the children-row for the live progress bar. The gap matches
  // StepContainer's `gap-2` (== distanceBetweenHeaderAndContent) so
  // ELK's offset and the rendered card height stay in lockstep —
  // see `nodeSize.ts::computeVerticalContainerSize` for the matching
  // height reservation.
  const extraTopHeight =
    nodeType === TaskType.LOOP_SEQUENTIAL
      ? loopProgressBarHeight + distanceBetweenHeaderAndContent
      : 0;
  const topPadding =
    headerHeight +
    nodePadding +
    nodeBorder +
    extraTopHeight +
    (childCount > 0 ? distanceBetweenHeaderAndContent + contentPadding : 0);
  const sidePadding = contentPadding + nodePadding + nodeBorder;

  return { top: topPadding, side: sidePadding };
};

const createLayoutOptions = (node: Node, padding: { top: number; side: number }) => {
  return {
    "elk.algorithm": "layered",
    "elk.direction": node.type === "conditional" ? "RIGHT" : "DOWN",
    "elk.padding": `[top=${padding.top}, left=${padding.side}, bottom=${padding.side}, right=${padding.side}]`,
    "elk.spacing.nodeNode": `${distanceBetweenNodes}`,
    "elk.nodeSize.constraints": "MINIMUM_SIZE",
    "elk.layered.spacing.nodeNodeBetweenLayers": `${distanceBetweenNodes}`,
    "elk.nodeSize.minimum": `(${node.width},${node.height})`
  };
};
