// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import useAutomation, { type TaskConfig, type TaskNode, TaskType } from "@/stores/useAutomation";

// One ELK run per graph, settled by the test: keyed by the graph's first node.
type Deferred = { resolve: (nodes: TaskNode[]) => void; reject: (error: Error) => void };
const layouts = vi.hoisted(() => new Map<string, Deferred>());
vi.mock(".", () => ({
  calculateNodesSize: (nodes: TaskNode[]) => nodes,
  getLayoutedElements: (nodes: TaskNode[]) =>
    nodes.length === 0
      ? Promise.resolve([])
      : new Promise<TaskNode[]>((resolve, reject) => {
          layouts.set(nodes[0].id, { resolve, reject });
        })
}));

import { useAutomationLayout } from "./useAutomationLayout";

const tasksNamed = (name: string): TaskConfig[] => [
  { name, type: TaskType.EXECUTE_SQL, database: "warehouse" }
];
const oldTasks = tasksNamed("old");
const newTasks = tasksNamed("new");

const laidOut = (id: string) => [{ id, position: { x: 1, y: 2 } }] as TaskNode[];

/** The ELK run for the graph starting at `id`, once the hook has asked for it. */
const requestedLayout = (id: string) =>
  waitFor(() => {
    const layout = layouts.get(id);
    if (!layout) throw new Error(`no layout requested for "${id}" yet`);
    return layout;
  });

/** Every promise callback already queued has run by the time this resolves. */
const settled = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

const renderLayout = (tasks: TaskConfig[]) =>
  renderHook(({ tasks }) => useAutomationLayout("automation", tasks), {
    initialProps: { tasks }
  });

/** Old tasks replaced by new ones, whose layout has landed; the old layout is still running. */
const replaceTasksMidLayout = async () => {
  const { result, rerender } = renderLayout(oldTasks);
  const oldLayout = await requestedLayout("old");

  rerender({ tasks: newTasks });
  const newLayout = await requestedLayout("new");
  act(() => newLayout.resolve(laidOut("new")));
  await waitFor(() => expect(result.current.nodes).toEqual(laidOut("new")));

  return { result, oldLayout };
};

beforeEach(() => {
  layouts.clear();
  useAutomation.setState({ baseNodes: [], nodes: [], edges: [] });
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  // Unmount, or a hook left over from one test lays out the next test's graph too.
  cleanup();
  vi.restoreAllMocks();
});

describe("useAutomationLayout", () => {
  it("reports a layout ELK rejects, and clears the report once a layout succeeds", async () => {
    const { result, rerender } = renderLayout(oldTasks);
    const oldLayout = await requestedLayout("old");
    expect(result.current.layoutFailed).toBe(false);

    act(() => oldLayout.reject(new Error("Referenced shape does not exist")));
    await waitFor(() => expect(result.current.layoutFailed).toBe(true));

    rerender({ tasks: newTasks });
    const newLayout = await requestedLayout("new");
    act(() => newLayout.resolve(laidOut("new")));
    await waitFor(() => expect(result.current.layoutFailed).toBe(false));
    expect(result.current.nodes).toEqual(laidOut("new"));
  });

  it("drops a layout that finishes after its graph was replaced", async () => {
    const { result, oldLayout } = await replaceTasksMidLayout();

    // The layout for the old tasks lands last. It must not replace the new one.
    await act(async () => {
      oldLayout.resolve(laidOut("old"));
      await settled();
    });
    expect(result.current.nodes).toEqual(laidOut("new"));
  });

  it("ignores the failure of a layout whose graph was replaced", async () => {
    const { result, oldLayout } = await replaceTasksMidLayout();

    await act(async () => {
      oldLayout.reject(new Error("too late"));
      await settled();
    });
    expect(result.current.layoutFailed).toBe(false);
  });
});
