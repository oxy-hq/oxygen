import type { Edge } from "@xyflow/react";
import {
  type ConditionalTaskConfigWithId,
  type NodeType,
  NoneTaskNodeType,
  type TaskConfigWithId,
  type TaskNode,
  TaskType
} from "@/stores/useAutomation";

interface NodeBuilderResult {
  nodes: TaskNode[];
  edges: Edge[];
}

const createNode = ({
  nodeId,
  task,
  index,
  metadata,
  parentId,
  type
}: {
  nodeId: string;
  task: TaskConfigWithId;
  index: number;
  metadata?: Record<string, unknown>;
  parentId?: string;
  name?: string;
  type?: NodeType;
}): TaskNode => {
  return {
    id: nodeId,
    data: {
      task: { ...task, id: nodeId },
      id: nodeId,
      index,
      metadata,
      expanded: task.type === "loop_sequential"
    },
    type: type ?? task.type,
    parentId,
    width: 0,
    height: 0
  } as TaskNode;
};

const createConditionalIfNode = (
  task: ConditionalTaskConfigWithId & { id: string; automationId: string },
  condition: { if: string },
  index: number,
  conditionIndex: number
): TaskNode => {
  const nodeId = `${task.id}-condition-${conditionIndex}`;
  return createNode({
    nodeId,
    task,
    index,
    metadata: {
      condition: condition.if
    },
    type: NoneTaskNodeType.CONDITIONAL_IF,
    parentId: task.id,
    name: condition.if
  });
};

const createConditionalElseNode = (
  task: ConditionalTaskConfigWithId & { id: string; automationId: string },
  index: number
): TaskNode => {
  const nodeId = `${task.id}-else`;
  return createNode({
    nodeId,
    task,
    index,
    parentId: task.id,
    type: NoneTaskNodeType.CONDITIONAL_ELSE,
    name: "Else"
  });
};

const buildConditionalNodes = (
  task: ConditionalTaskConfigWithId & { id: string; automationId: string },
  index: number,
  level: number
): { nodes: TaskNode[]; edges: Edge[] } => {
  const result: NodeBuilderResult = { nodes: [], edges: [] };

  task.conditions.forEach((condition, condIndex) => {
    const ifNode = createConditionalIfNode(task, condition, index, condIndex);
    result.nodes.push(ifNode);

    const { nodes, edges } = buildAutomationNodes(condition.tasks, ifNode.id, level + 1);
    result.nodes.push(...nodes);
    result.edges.push(...edges);

    // Each branch follows the one before it: the previous *condition*, which is
    // not the task's own place in its list (`index`).
    if (condIndex > 0) {
      result.edges.push({
        id: `${task.id}-condition-${condIndex - 1}-${task.id}-condition-${condIndex}`,
        source: `${task.id}-condition-${condIndex - 1}`,
        target: ifNode.id
      });
    }
  });

  if (task.else) {
    const elseNode = createConditionalElseNode(task, index);
    result.nodes.push(elseNode);

    const { nodes, edges } = buildAutomationNodes(task.else, elseNode.id, level + 1);
    result.nodes.push(...nodes);
    result.edges.push(...edges);

    if (task.conditions.length > 0) {
      result.edges.push({
        id: `${task.id}-condition-${task.conditions.length - 1}-${task.id}-else`,
        source: `${task.id}-condition-${task.conditions.length - 1}`,
        target: elseNode.id
      });
    }
  }

  return {
    nodes: result.nodes,
    edges: result.edges
  };
};

export const buildAutomationNodes = (
  tasks: TaskConfigWithId[],
  parentId?: string,
  level = 0
): NodeBuilderResult => {
  const result: NodeBuilderResult = { nodes: [], edges: [] };

  tasks.forEach((task, index) => {
    const node = createNode({
      nodeId: task.id,
      index,
      task,
      parentId
    });

    result.nodes.push(node);

    if (task.type === TaskType.LOOP_SEQUENTIAL) {
      const { nodes, edges } = buildAutomationNodes(task.tasks, node.id, level + 1);
      result.nodes.push(...nodes);
      result.edges.push(...edges);
    } else if (task.type === TaskType.WORKFLOW) {
      const { nodes, edges } = buildAutomationNodes(task.tasks ?? [], node.id, level + 1);
      result.nodes.push(...nodes.map((n) => ({ ...n })));
      result.edges.push(...edges);
    } else if (task.type === TaskType.CONDITIONAL) {
      const { nodes, edges } = buildConditionalNodes(task, index, level + 1);
      result.nodes.push(...nodes);
      result.edges.push(...edges);
    }

    if (index > 0) {
      const prevId = tasks[index - 1].id;
      result.edges.push({
        id: `${prevId}-${task.id}`,
        source: prevId,
        target: task.id
      });
    }
  });

  return {
    nodes: result.nodes,
    edges: sortEdges(result.edges)
  };
};

const sortEdges = (edges: Edge[]): Edge[] => edges.sort((a, b) => a.id.length - b.id.length);
