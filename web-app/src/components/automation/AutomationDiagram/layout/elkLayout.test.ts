import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { type TaskConfigWithId, TaskType } from "@/stores/useAutomation";
import { calculateNodesSize, getLayoutedElements } from ".";
import { buildAutomationNodes } from "./nodeBuilder";

const step = (id: string): TaskConfigWithId => ({
  id,
  name: id,
  automationId: "automation",
  type: TaskType.EXECUTE_SQL,
  database: "warehouse"
});

/** Lays out these tasks with these extra edges, as the diagram does. */
const layOut = (
  tasks: TaskConfigWithId[],
  extraEdges: { id: string; source: string; target: string }[]
) => {
  const { nodes, edges } = buildAutomationNodes(tasks);
  return getLayoutedElements(calculateNodesSize(nodes), [...edges, ...extraEdges]);
};

let warn: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  warn = vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("createElkLayout", () => {
  it("lays out the rest of the graph without an edge to a node that does not exist, and says so", async () => {
    const laidOut = await layOut(
      [step("a"), step("b")],
      [
        { id: "b-ghost", source: "b", target: "ghost" },
        { id: "phantom-a", source: "phantom", target: "a" }
      ]
    );

    // ELK rejects the whole graph over one such edge, so it is left out...
    expect(laidOut.map((node) => node.id)).toEqual(["a", "b"]);
    // ...but not silently: a dangling edge is a bug in whatever built the graph.
    expect(warn).toHaveBeenCalledTimes(1);
    const [message] = warn.mock.calls[0] as [string];
    expect(message).toContain("b-ghost");
    expect(message).toContain("phantom-a");
  });

  it("says nothing of the edges inside a closed container", async () => {
    // A conditional starts closed: its branches, and the edge between them, are
    // not drawn, but every one of them is there.
    await layOut(
      [
        step("first"),
        {
          id: "check",
          name: "check",
          automationId: "automation",
          type: TaskType.CONDITIONAL,
          conditions: [
            { if: "ready", tasks: [step("check.yes")] },
            { if: "late", tasks: [step("check.later")] }
          ],
          else: [step("check.no")]
        }
      ],
      []
    );

    expect(warn).not.toHaveBeenCalled();
  });
});
