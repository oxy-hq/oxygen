// @vitest-environment jsdom

import { act, cleanup, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LineageOutput } from "@/types/modeling";
import LineageGraph from "./LineageGraph";

// The layout is asynchronous, so picking a second node while the first one's layout is
// still running leaves two in flight. The older one belongs to a selection that is gone:
// neither its result, its failure nor its "finished" may land on the newer one.

type ElkGraph = { children: { id: string }[] };
type Deferred = { graph: ElkGraph; resolve: () => void; reject: (error: Error) => void };

const { layouts, lineage } = vi.hoisted(() => ({
  layouts: [] as Deferred[],
  lineage: { data: undefined as LineageOutput | undefined }
}));

vi.mock("elkjs", () => ({
  default: class {
    layout(graph: ElkGraph) {
      return new Promise((resolve, reject) => {
        layouts.push({ graph, resolve: () => resolve({ children: graph.children }), reject });
      });
    }
  }
}));
vi.mock("@/hooks/api/modeling/useModelingLineage", () => ({
  default: () => ({ data: lineage.data, isLoading: false, error: null })
}));
// React Flow measures the DOM, which jsdom cannot do; what matters here is which nodes
// it was handed.
vi.mock("@xyflow/react", () => ({
  ReactFlowProvider: ({ children }: { children: ReactNode }) => children,
  ReactFlow: ({ nodes }: { nodes: { id: string }[] }) => (
    <div data-testid='flow'>{nodes.map((n) => n.id).join(",")}</div>
  ),
  Background: () => null,
  Handle: () => null,
  BackgroundVariant: { Dots: "dots" },
  Position: { Left: "left", Right: "right" }
}));

const node = (id: string) => ({
  unique_id: id,
  name: id,
  resource_type: "model",
  description: null,
  path: null
});

// Two unconnected chains, so each selection lays out a different set of nodes.
const DATA: LineageOutput = {
  nodes: [node("a"), node("a2"), node("b"), node("b2")],
  edges: [
    { source: "a", target: "a2" },
    { source: "b", target: "b2" }
  ]
};

const settle = async (fn: () => void) => {
  await act(async () => {
    fn();
    await Promise.resolve();
  });
};

beforeEach(() => {
  layouts.length = 0;
  lineage.data = DATA;
  vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => cleanup());

describe("LineageGraph layout", () => {
  it("does not show a previous node's layout failure over the current node's graph", async () => {
    const { rerender } = render(<LineageGraph nodeId='a' dbtProjectName='dbt' />);
    rerender(<LineageGraph nodeId='b' dbtProjectName='dbt' />);
    expect(layouts).toHaveLength(2);

    await settle(() => layouts[0].reject(new Error("unknown node")));
    await settle(() => layouts[1].resolve());

    expect(screen.queryByText("Failed to load lineage")).toBeNull();
    expect(screen.getByTestId("flow").textContent).toBe("b,b2");
  });

  it("keeps the spinner until the current node's layout lands", async () => {
    const { rerender, container } = render(<LineageGraph nodeId='a' dbtProjectName='dbt' />);
    rerender(<LineageGraph nodeId='b' dbtProjectName='dbt' />);

    // The previous node's layout finishes first: its graph must not appear under "b".
    await settle(() => layouts[0].resolve());
    expect(screen.queryByTestId("flow")).toBeNull();
    expect(container.querySelector(".animate-spin")).not.toBeNull();

    await settle(() => layouts[1].resolve());
    expect(screen.getByTestId("flow").textContent).toBe("b,b2");
  });

  it("still reports a failure of the current node's layout", async () => {
    render(<LineageGraph nodeId='a' dbtProjectName='dbt' />);

    await settle(() => layouts[0].reject(new Error("unknown node")));

    expect(screen.getByText("Failed to load lineage")).toBeTruthy();
  });

  it("stops the spinner when the lineage goes away mid-layout", async () => {
    const { rerender, container } = render(<LineageGraph nodeId='a' dbtProjectName='dbt' />);

    // e.g. the dbt project changed and its lineage has not loaded.
    lineage.data = undefined;
    rerender(<LineageGraph nodeId='a' dbtProjectName='other' />);
    await settle(() => layouts[0].resolve());

    expect(container.querySelector(".animate-spin")).toBeNull();
  });
});
