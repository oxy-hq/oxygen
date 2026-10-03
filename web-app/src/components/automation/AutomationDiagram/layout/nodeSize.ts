import {
  type TaskNode as Node,
  type NodeType,
  NoneTaskNodeType,
  TaskType
} from "@/stores/useAutomation";
import {
  contentPadding,
  contentPaddingHeight,
  distanceBetweenHeaderAndContent,
  distanceBetweenNodes,
  headerHeight,
  loopProgressBarHeight,
  minNodeWidth,
  nodeBorder,
  nodeBorderHeight,
  nodePadding,
  normalNodeHeight,
  paddingHeight,
  smallestNodeWidth
} from "./constants";

export const computeNodeDimensions = (node: Node, allNodes: Node[]): void => {
  if (isSpecialNode(node.type)) {
    computeSpecialNodeSize(node, allNodes);
  } else {
    computeBasicNodeSize(node);
  }
};

const isSpecialNode = (type: NodeType): boolean => {
  return [
    NoneTaskNodeType.CONDITIONAL_ELSE,
    NoneTaskNodeType.CONDITIONAL_IF,
    TaskType.LOOP_SEQUENTIAL,
    TaskType.CONDITIONAL,
    TaskType.WORKFLOW
  ].includes(type);
};

const computeBasicNodeSize = (node: Node): void => {
  node.width = smallestNodeWidth;
  node.height = normalNodeHeight;
};

const computeSpecialNodeSize = (node: Node, allNodes: Node[]): void => {
  switch (node.type) {
    case NoneTaskNodeType.CONDITIONAL_ELSE:
    case NoneTaskNodeType.CONDITIONAL_IF:
    case TaskType.LOOP_SEQUENTIAL:
    case TaskType.WORKFLOW: {
      const verticalLayout = computeVerticalContainerSize(node, allNodes);
      node.width = verticalLayout.width;
      node.height = verticalLayout.height;
      break;
    }
    case TaskType.CONDITIONAL: {
      const horizontalLayout = computeHorizontalContainerSize(node, allNodes);
      node.width = horizontalLayout.width;
      node.height = horizontalLayout.height;
      break;
    }
    default:
      // Not reached: isSpecialNode() admits only the five container types
      // above. Every leaf task type is sized by computeBasicNodeSize.
      break;
  }
};

const computeVerticalContainerSize = (
  node: Node,
  allNodes: Node[]
): { width: number; height: number } => {
  const children = getVisibleChildren(node, allNodes);

  let totalHeight = 0;
  let maxWidth = minNodeWidth;

  children.forEach((child, index) => {
    if (child.width === 0) computeNodeDimensions(child, allNodes);
    maxWidth = Math.max(maxWidth, child.width || 0);
    totalHeight += (child.height || 0) + (index > 0 ? distanceBetweenNodes : 0);
  });

  // Loop nodes reserve the bar's own height + the flex `gap-2`
  // that sits between `NodeHeader` and the bar inside StepContainer.
  // Without the gap, the status-indicator border (which tracks
  // `node.height`) ends up 8px short of the visible card because
  // gap-2 is 8px and StepContainer renders header+bar with one gap
  // between them — invisible to the layout engine otherwise.
  const extraTopHeight =
    node.type === TaskType.LOOP_SEQUENTIAL
      ? loopProgressBarHeight + distanceBetweenHeaderAndContent
      : 0;
  return calculateContainerDimensions(maxWidth, totalHeight, children.length, extraTopHeight);
};

const computeHorizontalContainerSize = (
  node: Node,
  allNodes: Node[]
): { width: number; height: number } => {
  const children = getVisibleChildren(node, allNodes);

  let totalWidth = 0;
  let maxHeight = 0;

  children.forEach((child, index) => {
    if (child.width === 0) computeNodeDimensions(child, allNodes);
    maxHeight = Math.max(maxHeight, child.height || 0);
    totalWidth += (child.width || 0) + (index > 0 ? distanceBetweenNodes : 0);
  });

  return calculateContainerDimensions(totalWidth, maxHeight, children.length);
};

/**
 * Make each child of a vertical container (a loop, a workflow, a conditional's
 * branch) as wide as the container's inside, from `node` down, as the top-level
 * nodes are all made as wide as the widest. A container is sized from its
 * children's own widths first, and only then is its final width known: it is
 * widened by a wider sibling, or by the top-level column. Left at their own
 * width, its steps sat narrow inside it, out of line with a wider child.
 * A conditional's branches sit side by side, so they keep their own widths.
 */
export const fillContainerWidths = (node: Node, allNodes: Node[]): void => {
  const children = getVisibleChildren(node, allNodes);
  if (children.length === 0) return;
  if (node.type !== TaskType.CONDITIONAL) {
    const inside = (node.width || 0) - 2 * (contentPadding + nodePadding + nodeBorder);
    children.forEach((child) => {
      child.width = inside;
    });
  }
  children.forEach((child) => fillContainerWidths(child, allNodes));
};

const getVisibleChildren = (node: Node, allNodes: Node[]): Node[] => {
  return node.data.expanded ? allNodes.filter((n) => n.parentId === node.id) : [];
};

const calculateContainerDimensions = (
  baseWidth: number,
  baseHeight: number,
  childCount: number,
  extraTopHeight = 0
): { width: number; height: number } => {
  let width = baseWidth;
  let height = baseHeight;

  height += headerHeight + paddingHeight + nodeBorderHeight + extraTopHeight;

  if (childCount > 0) {
    width += 2 * (contentPadding + nodePadding + nodeBorder);
    height += distanceBetweenHeaderAndContent + contentPaddingHeight;
  }

  return { width, height };
};
