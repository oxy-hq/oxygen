import { describe, expect, it } from "vitest";
import { type TaskConfigWithId, type TaskNode, TaskType } from "@/stores/useAutomation";
import { calculateNodesSize, getLayoutedElements } from ".";
import {
  contentPadding,
  distanceBetweenNodes,
  nodeBorder,
  nodePadding,
  normalNodeHeight
} from "./constants";
import { buildAutomationNodes } from "./nodeBuilder";

/** The room a container leaves on each side of its children. */
const sidePadding = contentPadding + nodePadding + nodeBorder;

const base = (id: string) => ({ id, name: id, automationId: "automation" });

const step = (id: string): TaskConfigWithId => ({
  ...base(id),
  type: TaskType.EXECUTE_SQL,
  database: "warehouse"
});

const loop = (id: string, tasks: TaskConfigWithId[]): TaskConfigWithId => ({
  ...base(id),
  type: TaskType.LOOP_SEQUENTIAL,
  values: [],
  tasks
});

const conditional = (
  id: string,
  ifTasks: TaskConfigWithId[],
  elseTasks: TaskConfigWithId[]
): TaskConfigWithId => ({
  ...base(id),
  type: TaskType.CONDITIONAL,
  conditions: [{ if: "ready", tasks: ifTasks }],
  else: elseTasks
});

/** Sizes as computed up front, and as ELK lays the same graph out, all containers open. */
const sizeAndLayOut = async (tasks: TaskConfigWithId[]) => {
  const { nodes, edges } = buildAutomationNodes(tasks);
  const expanded = nodes.map((node) => ({ ...node, data: { ...node.data, expanded: true } }));
  const sized = calculateNodesSize(expanded);
  const laidOut = await getLayoutedElements(sized, edges);
  const byId = (list: TaskNode[], id: string) => {
    const found = list.find((node) => node.id === id);
    if (!found) throw new Error(`no node "${id}"`);
    return found;
  };
  return {
    sized: (id: string) => byId(sized, id),
    laidOut: (id: string) => byId(laidOut, id),
    ids: sized.map((node) => node.id)
  };
};

describe("container node size", () => {
  it("makes a vertical container as tall as its children plus the gaps between them", async () => {
    const { sized, laidOut } = await sizeAndLayOut([
      loop("loop", [step("loop.a"), step("loop.b"), step("loop.c")])
    ]);

    // ELK puts each child one gap below the previous one...
    const [a, b, c] = ["loop.a", "loop.b", "loop.c"].map((id) => laidOut(id).position.y);
    expect(b - a).toBe(normalNodeHeight + distanceBetweenNodes);
    expect(c - b).toBe(normalNodeHeight + distanceBetweenNodes);
    // ...so the container has to have room for both gaps.
    expect(sized("loop").height).toBe(laidOut("loop").height);
    expect(sized("loop").height).toBe(326);
  });

  it("makes a conditional as wide as its branches plus the gap between them", async () => {
    const { sized, laidOut } = await sizeAndLayOut([
      step("first"),
      conditional("check", [step("check.yes")], [step("check.no")])
    ]);

    const ifBranch = laidOut("check-condition-0");
    const elseBranch = laidOut("check-else");
    expect(elseBranch.position.x - ifBranch.position.x).toBe(
      (ifBranch.width ?? 0) + distanceBetweenNodes
    );
    expect(sized("check").width).toBe(laidOut("check").width);
    expect(sized("check").width).toBe(562);
  });

  it("keeps top-level nodes the same width when one of them holds a conditional", async () => {
    // Top-level nodes are all given the widest one's width. That only holds if
    // ELK does not then have to widen a container to fit children that were
    // measured without their gaps.
    const { laidOut } = await sizeAndLayOut([
      step("first"),
      loop("loop", [conditional("loop.check", [step("loop.check.yes")], [step("loop.check.no")])]),
      step("last")
    ]);

    const widths = ["first", "loop", "last"].map((id) => laidOut(id).width);
    expect(widths).toEqual([596, 596, 596]);
  });

  it("computes for every node the size ELK lays it out at", async () => {
    const { sized, laidOut, ids } = await sizeAndLayOut([
      step("first"),
      conditional("check", [step("check.a"), step("check.b")], [step("check.c")]),
      loop("loop", [
        step("loop.a"),
        conditional("loop.check", [step("loop.check.yes")], [step("loop.check.no")]),
        step("loop.c")
      ])
    ]);

    const size = (node: TaskNode) => ({ id: node.id, width: node.width, height: node.height });
    expect(ids.map((id) => size(sized(id)))).toEqual(ids.map((id) => size(laidOut(id))));
  });

  /** Where each child sits across its container, and how wide it is. */
  const columnOf = (laidOut: (id: string) => TaskNode, ids: string[]) =>
    ids.map((id) => ({ id, x: laidOut(id).position.x, width: laidOut(id).width }));

  it("makes every child of a vertical container as wide as the container's inside", async () => {
    const { laidOut } = await sizeAndLayOut([
      loop("loop", [
        step("loop.a"),
        conditional("loop.check", [step("loop.check.yes")], [step("loop.check.no")]),
        step("loop.c")
      ])
    ]);

    // One column, as the top-level nodes are: the steps used to stay 200 wide
    // beside a 562-wide conditional, one gap in from the container's left edge.
    const inside = (laidOut("loop").width ?? 0) - 2 * sidePadding;
    expect(columnOf(laidOut, ["loop.a", "loop.check", "loop.c"])).toEqual([
      { id: "loop.a", x: sidePadding, width: inside },
      { id: "loop.check", x: sidePadding, width: inside },
      { id: "loop.c", x: sidePadding, width: inside }
    ]);
  });

  it("widens a container's children with it when the top-level column widens it", async () => {
    const { laidOut } = await sizeAndLayOut([
      loop("loop", [step("loop.a"), step("loop.b")]),
      conditional("check", [step("check.yes")], [step("check.no")])
    ]);

    // The loop is as wide as the conditional below it; its steps fill it.
    expect(laidOut("loop").width).toBe(laidOut("check").width);
    const inside = (laidOut("loop").width ?? 0) - 2 * sidePadding;
    expect(columnOf(laidOut, ["loop.a", "loop.b"])).toEqual([
      { id: "loop.a", x: sidePadding, width: inside },
      { id: "loop.b", x: sidePadding, width: inside }
    ]);
  });
});
