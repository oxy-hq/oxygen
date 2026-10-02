import { describe, expect, it } from "vitest";
import { type TaskConfigWithId, TaskType } from "@/stores/useAutomation";
import { calculateNodesSize, getLayoutedElements } from ".";
import { buildAutomationNodes } from "./nodeBuilder";

const base = (id: string) => ({ id, name: id, automationId: "automation" });

const step = (id: string): TaskConfigWithId => ({
  ...base(id),
  type: TaskType.EXECUTE_SQL,
  database: "warehouse"
});

/** A conditional with three `if` branches and an `else`, one step in each. */
const threeBranches = (id: string): TaskConfigWithId => ({
  ...base(id),
  type: TaskType.CONDITIONAL,
  conditions: ["small", "medium", "large"].map((size) => ({
    if: size,
    tasks: [step(`${id}.${size}`)]
  })),
  else: [step(`${id}.other`)]
});

/** A task list with the conditional `stepsBefore` steps down it. */
const tasksWithConditionalAt = (stepsBefore: number): TaskConfigWithId[] => [
  ...Array.from({ length: stepsBefore }, (_, i) => step(`before-${i}`)),
  threeBranches("route")
];

const branchIds = ["route-condition-0", "route-condition-1", "route-condition-2", "route-else"];

describe("conditional branches", () => {
  it.each([
    ["first in its task list", 0],
    ["second in its task list", 1],
    ["fourth in its task list", 3]
  ])("chains the branches of a conditional that is %s", (_position, stepsBefore) => {
    const { nodes, edges } = buildAutomationNodes(tasksWithConditionalAt(stepsBefore));

    const nodeIds = new Set(nodes.map((node) => node.id));
    for (const edge of edges) {
      expect(nodeIds).toContain(edge.source);
      expect(nodeIds).toContain(edge.target);
    }

    const betweenBranches = edges
      .filter((edge) => branchIds.includes(edge.target))
      .map((edge) => `${edge.source} -> ${edge.target}`);
    expect(betweenBranches.sort()).toEqual([
      "route-condition-0 -> route-condition-1",
      "route-condition-1 -> route-condition-2",
      "route-condition-2 -> route-else"
    ]);
    expect(new Set(edges.map((edge) => edge.id)).size).toBe(edges.length);
  });

  it.each([
    ["first in its task list", 0],
    ["fourth in its task list", 3]
  ])(
    "lays the branches out in one row, in order, when it is %s",
    async (_position, stepsBefore) => {
      const { nodes, edges } = buildAutomationNodes(tasksWithConditionalAt(stepsBefore));
      const expanded = nodes.map((node) => ({ ...node, data: { ...node.data, expanded: true } }));
      const laidOut = await getLayoutedElements(calculateNodesSize(expanded), edges);

      const branches = branchIds.map((id) => {
        const node = laidOut.find((n) => n.id === id);
        if (!node) throw new Error(`no node "${id}"`);
        return node.position;
      });
      expect(new Set(branches.map((position) => position.y)).size).toBe(1);
      const xs = branches.map((position) => position.x);
      expect(xs).toEqual([...xs].sort((a, b) => a - b));
      expect(new Set(xs).size).toBe(xs.length);
    }
  );
});
