// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DataFlow, DataFlowNode } from "@/types/dataFlow";
import type { DataFlowParams } from "../useDataFlowParams";

// The canvas itself is a stub: one button per drawn node, and what it was told is selected.
vi.mock("./DataFlowGraph", () => ({
  DataFlowGraph: ({
    flow,
    selected,
    onSelect
  }: {
    flow: { nodes: DataFlowNode[] };
    selected: string | null;
    onSelect: (id: string) => void;
  }) => (
    <div>
      <output data-testid='stub-selected'>{selected ?? "none"}</output>
      {flow.nodes.map((n) => (
        <button key={n.id} type='button' onClick={() => onSelect(n.id)}>
          {n.id}
        </button>
      ))}
    </div>
  )
}));
vi.mock("./DataFlowViewBar", () => ({ DataFlowViewBar: () => null }));
vi.mock("./NodeDetails", () => ({
  NodeDetails: ({ node }: { node: DataFlowNode }) => <p>{`details of ${node.id}`}</p>,
  NodeDetailsEmpty: () => <p>Click a node</p>
}));

import { DataFlowCanvas } from "./DataFlowCanvas";

const app = (id: string): DataFlowNode => ({
  id,
  type: "app",
  label: id,
  meta: { workspace_id: "w" }
});

const flow: DataFlow = {
  generated_at: "2026-11-25T00:00:00Z",
  org: { id: "o", name: "Org", slug: "org" },
  workspaces: [],
  truncated: false,
  level: "schema",
  nodes: ["app:a", "app:b", "app:c", "app:d"].map(app),
  edges: []
};

const params = (open: DataFlowParams["open"]): DataFlowParams => ({
  org: "org",
  workspace: null,
  focus: null,
  depth: 2,
  level: "schema",
  hide: [],
  q: "",
  open
});

afterEach(cleanup);

describe("DataFlowCanvas", () => {
  it("selects nothing once the selected node folds away", () => {
    const props = { flow, placeholder: false, onChange: vi.fn() };
    const { rerender } = render(<DataFlowCanvas {...props} params={params("all")} />);
    fireEvent.click(screen.getByText("app:a"));
    expect(screen.getByTestId("stub-selected").textContent).toBe("app:a");
    expect(screen.getByText("details of app:a")).toBeTruthy();

    // Collapse all: app:a is now inside the folded apps card.
    rerender(<DataFlowCanvas {...props} params={params([])} />);
    expect(screen.queryByText("app:a")).toBeNull();
    // Not still selected, so the canvas is not dimmed around a node it no longer draws.
    expect(screen.getByTestId("stub-selected").textContent).toBe("none");
    expect(screen.getByText("Click a node")).toBeTruthy();
  });
});
