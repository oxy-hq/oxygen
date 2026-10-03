import type { Edge } from "@xyflow/react";
import type { TaskNode } from "@/stores/useAutomation";
import { createElkLayout } from "./elkLayout";
import { computeNodeDimensions, fillContainerWidths } from "./nodeSize";

export const calculateNodesSize = (nodes: TaskNode[]): TaskNode[] => {
  const nodesWithSize = nodes.map((node) => ({ ...node }));

  nodesWithSize.forEach((node) => {
    computeNodeDimensions(node, nodesWithSize);
  });

  const maxWidth = Math.max(...nodesWithSize.map((node) => node.width || 0));
  nodesWithSize
    .filter((node) => !node.parentId)
    .forEach((node) => {
      node.width = maxWidth;
      fillContainerWidths(node, nodesWithSize);
    });

  return nodesWithSize;
};

export const getLayoutedElements = async (
  nodes: TaskNode[],
  edges: Edge[]
): Promise<TaskNode[]> => {
  return createElkLayout(nodes, edges);
};
